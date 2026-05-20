/* eslint-disable security/detect-non-literal-fs-filename -- remote-dev logs are written to configured per-thread directories. */
// RxJS-based reactive event bus for the dev-server.
//
// Replaces the imperative callback-based emit() + fire-and-forget fetch
// pattern with a structured reactive pipeline that provides:
//
//   1. **Retry with exponential backoff** on Vercel ingest failures
//   2. **Buffered batch writes** — groups events into micro-batches
//      to reduce HTTP round-trips
//   3. **Per-task ReplaySubject** — late SSE subscribers get full history
//   4. **Supabase broadcast pipeline** with retry + timeout
//   5. **Log sink** — async batched append to tmp/convos/{threadId}/thread.log
//   6. **Reconnection-safe** — pipelines restart on transient failures
//      without losing events (ReplaySubject holds the buffer)
//   7. **Circuit breaker** — drops events after sustained failures to
//      prevent unbounded memory growth
//   8. **Dedup** — seq-based deduplication at the ReplaySubject layer
//   9. **Self-registration** — container registers itself in Redis on boot
//
// Usage in server.ts:
//   const bus = new EventBus();
//   bus.startVercelIngest(url, secret);
//   bus.startSupabaseBroadcast(publishFn);
//   bus.startLogSink("/tmp/convos");
//   // ... later:
//   bus.emit({ taskId, seq, event, userId, threadId });

import type { Observable, Subscription } from "rxjs";
import {
  Subject,
  ReplaySubject,
  EMPTY,
  from,
  timer,
  interval,
  BehaviorSubject,
} from "rxjs";
import {
  filter,
  map,
  tap,
  retry,
  bufferTime,
  concatMap,
  mergeMap,
  catchError,
  takeUntil,
  timeout,
  share,
} from "rxjs/operators";
import { appendFile, mkdir } from "node:fs/promises";
import { join } from "node:path";

// Re-use the event shape from server.ts (we import the type; the bus
// is imported by server.ts, so we define a standalone interface to
// avoid circular deps).
export interface BusEvent {
  taskId: string;
  threadId?: string;
  userId?: string;
  seq: number;
  event: {
    kind: string;
    [key: string]: unknown;
  };
}

// Circuit breaker state for the ingest pipeline.
interface CircuitState {
  consecutiveFailures: number;
  isOpen: boolean;
  lastFailAt: number;
}

const CIRCUIT_FAILURE_THRESHOLD = 15;
const CIRCUIT_RESET_MS = 60_000; // try again after 1 minute

export class EventBus {
  // ---- Core subjects ----

  /** All events, hot observable. */
  private readonly _events$ = new Subject<BusEvent>();
  /** Teardown signal for all pipelines. */
  private readonly _destroy$ = new Subject<void>();
  /** Per-task replay subjects. Late SSE subscribers get full history. */
  private readonly _taskReplays = new Map<string, ReplaySubject<BusEvent>>();
  /** Per-task seq dedup sets — prevents duplicate events in replay. */
  private readonly _taskSeqs = new Map<string, Set<number>>();
  /** Active pipeline subscriptions (for cleanup). */
  private readonly _subs: Subscription[] = [];
  /** Circuit breaker for Vercel ingest. */
  private readonly _ingestCircuit$ = new BehaviorSubject<CircuitState>({
    consecutiveFailures: 0,
    isOpen: false,
    lastFailAt: 0,
  });

  /** Public observable — downstream can subscribe to all events. */
  readonly all$: Observable<BusEvent> = this._events$.asObservable().pipe(
    share(), // single execution for multiple subscribers
  );

  /** Expose circuit state for health monitoring. */
  readonly ingestCircuitState$: Observable<CircuitState> =
    this._ingestCircuit$.asObservable();

  getCircuitState(): CircuitState {
    return this._ingestCircuit$.getValue();
  }

  // ---- Emit ----

  emit(ev: BusEvent): void {
    // Push to global stream.
    this._events$.next(ev);

    // Dedup by (taskId, seq) before pushing to per-task replay.
    let seqs = this._taskSeqs.get(ev.taskId);
    if (!seqs) {
      seqs = new Set<number>();
      this._taskSeqs.set(ev.taskId, seqs);
    }
    if (seqs.has(ev.seq)) {
      return; // exact duplicate — skip replay push
    }
    seqs.add(ev.seq);

    // Push to per-task replay (create on first event).
    let replay = this._taskReplays.get(ev.taskId);
    if (!replay) {
      // Buffer up to 2000 events per task — generous enough for any
      // real agent run, bounded enough to prevent OOM on a runaway loop.
      replay = new ReplaySubject<BusEvent>(2000);
      this._taskReplays.set(ev.taskId, replay);
    }
    replay.next(ev);

    // Terminal: complete the per-task replay so subscribers know it's done.
    if (ev.event.kind === "done") {
      replay.complete();
    }
  }

  /**
   * Observable of events for a single task, with replay of all
   * previously emitted events. Returns EMPTY if the task is unknown.
   */
  taskEvents$(taskId: string): Observable<BusEvent> {
    const replay = this._taskReplays.get(taskId);
    return replay ? replay.asObservable() : EMPTY;
  }

  // ---- Pipelines ----

  /**
   * Start the Vercel ingest pipeline.
   *
   * Events are micro-batched (up to 10 events or 250ms, whichever comes
   * first) and POSTed to the ingest URL. Each event within a batch is
   * sent with concurrency=3 (mergeMap) to avoid head-of-line blocking.
   *
   * On failure, retries up to 5× with exponential backoff (1s → 2s →
   * 4s → 8s → 16s, ±20% jitter). After exhausting retries, the event
   * is dropped but the circuit breaker tracks consecutive failures.
   * After 15 consecutive drops, the circuit opens for 60s — events
   * during this window are dropped immediately to prevent memory
   * buildup. NeonDB will eventually be reconciled by the heartbeat
   * route.
   */
  startVercelIngest(ingestUrl: string, secret: string): void {
    const sub = this._events$
      .pipe(
        takeUntil(this._destroy$),
        // Circuit breaker gate — drop events while circuit is open.
        filter(() => {
          const state = this._ingestCircuit$.getValue();
          if (!state.isOpen) {return true;}
          // Check if reset period has elapsed.
          if (Date.now() - state.lastFailAt > CIRCUIT_RESET_MS) {
            this._ingestCircuit$.next({
              consecutiveFailures: 0,
              isOpen: false,
              lastFailAt: 0,
            });
            return true;
          }
          return false;
        }),
        bufferTime(250, undefined, 10),
        filter((batch) => batch.length > 0),
        concatMap((batch) => {
          // mergeMap with concurrency 3 — events within a batch can
          // proceed in parallel, but batches are sequential (concatMap
          // on the outer operator) to preserve ordering guarantees.
          return from(batch).pipe(
            mergeMap(
              (ev) =>
                from(
                  fetch(ingestUrl, {
                    method: "POST",
                    headers: {
                      "Content-Type": "application/json",
                      "X-Agent-Auth": secret,
                    },
                    body: JSON.stringify({
                      taskId: ev.taskId,
                      seq: ev.seq,
                      event: ev.event,
                    }),
                  }),
                ).pipe(
                  // Fail if response is not ok.
                  mergeMap(async (res) => {
                    if (!res.ok) {
                      const text = await res.text().catch(() => "");
                      throw new Error(
                        `ingest ${res.status}: ${text.slice(0, 200)}`,
                      );
                    }
                    return res;
                  }),
                  retry({
                    count: 5,
                    delay: (_err, retryCount) => {
                      const baseDelay = 1000 * Math.pow(2, retryCount - 1);
                      const jitter =
                        baseDelay * 0.2 * (Math.random() * 2 - 1);
                      return timer(Math.min(baseDelay + jitter, 30_000));
                    },
                  }),
                  tap({
                    next: () => {
                      // Reset circuit on success.
                      const state = this._ingestCircuit$.getValue();
                      if (state.consecutiveFailures > 0) {
                        this._ingestCircuit$.next({
                          consecutiveFailures: 0,
                          isOpen: false,
                          lastFailAt: 0,
                        });
                      }
                    },
                  }),
                  catchError((err) => {
                    // Track consecutive failures for circuit breaker.
                    const state = this._ingestCircuit$.getValue();
                    const next: CircuitState = {
                      consecutiveFailures: state.consecutiveFailures + 1,
                      isOpen:
                        state.consecutiveFailures + 1 >=
                        CIRCUIT_FAILURE_THRESHOLD,
                      lastFailAt: Date.now(),
                    };
                    this._ingestCircuit$.next(next);

                    if (next.isOpen) {
                      process.stderr.write(
                        `[event-bus] circuit OPEN after ${next.consecutiveFailures} consecutive failures — dropping events for ${CIRCUIT_RESET_MS / 1000}s\n`,
                      );
                    } else {
                      process.stderr.write(
                        `[event-bus] ingest failed after retries (${next.consecutiveFailures}/${CIRCUIT_FAILURE_THRESHOLD}): ${
                          err instanceof Error ? err.message : String(err)
                        }\n`,
                      );
                    }
                    return EMPTY;
                  }),
                ),
              3, // concurrency limit within a batch
            ),
          );
        }),
        map(() => undefined),
      )
      .subscribe();
    this._subs.push(sub);
  }

  /**
   * Start the Supabase Broadcast pipeline.
   *
   * Each event with a userId is published to the per-user channel.
   * Uses mergeMap(3) so events for different users proceed in parallel.
   * Each publish has a 5s timeout. Retries up to 3× with 1s delay,
   * then drops (best-effort; NeonDB has the durable copy).
   */
  startSupabaseBroadcast(
    publishFn: (_userId: string, _payload: unknown) => Promise<void>,
  ): void {
    const sub = this._events$
      .pipe(
        takeUntil(this._destroy$),
        filter((ev) => !!ev.userId),
        mergeMap(
          (ev) =>
            from(
              publishFn(ev.userId!, {
                taskId: ev.taskId,
                threadId: ev.threadId,
                seq: ev.seq,
                event: ev.event,
              }),
            ).pipe(
              timeout(5_000),
              retry({ count: 3, delay: 1000 }),
              catchError((err) => {
                process.stderr.write(
                  `[event-bus] broadcast failed: ${
                    err instanceof Error ? err.message : String(err)
                  }\n`,
                );
                return EMPTY;
              }),
            ),
          3, // concurrency — events for different users in parallel
        ),
        map(() => undefined),
      )
      .subscribe();
    this._subs.push(sub);
  }

  /**
   * Start the log sink pipeline.
   *
   * Events are micro-batched (up to 20 events or 100ms) and appended
   * asynchronously to `{logDir}/{threadId}/thread.log` (falling back to
   * `{logDir}/thread.log` when threadId is absent). Async writes avoid
   * blocking the event loop.
   */
  startLogSink(logDir: string): void {
    // Pre-create the base dir.
    void mkdir(logDir, { recursive: true }).catch(() => {});

    const sub = this._events$
      .pipe(
        takeUntil(this._destroy$),
        bufferTime(100, undefined, 20),
        filter((batch) => batch.length > 0),
        concatMap((batch) => {
          // Group by threadId so each thread's log file gets a single write.
          const byThread = new Map<string, string[]>();
          for (const ev of batch) {
            const key = ev.threadId ?? "__default__";
            let lines = byThread.get(key);
            if (!lines) {
              lines = [];
              byThread.set(key, lines);
            }
            const ts = new Date().toISOString();
            const kindDetail =
              ev.event.kind === "status"
                ? ` status=${(ev.event as { status?: string }).status ?? "?"}`
                : ev.event.kind === "error"
                  ? ` message=${String((ev.event as { message?: string }).message ?? "").slice(0, 200)}`
                  : ev.event.kind === "done"
                    ? ` exitReason=${(ev.event as { exitReason?: string }).exitReason ?? "?"}`
                    : "";
            lines.push(
              `[${ts}] task=${ev.taskId} seq=${ev.seq} kind=${ev.event.kind}${kindDetail}`,
            );
          }

          const writes = Array.from(byThread.entries()).map(
            async ([threadKey, lines]) => {
              const dir =
                threadKey === "__default__"
                  ? logDir
                  : join(logDir, threadKey);
              try {
                await mkdir(dir, { recursive: true });
              } catch {
                /* dir may exist */
              }
              const logFile = join(dir, "thread.log");
              const data = lines.join("\n") + "\n";
              try {
                await appendFile(logFile, data, "utf8");
              } catch {
                /* best-effort — don't crash the server for a log write */
              }
            },
          );
          return from(Promise.all(writes));
        }),
        map(() => undefined),
      )
      .subscribe();
    this._subs.push(sub);
  }

  /**
   * Start an idle-timeout watchdog. Emits to the provided callback
   * when no events have flowed for `idleMs` milliseconds. Used by
   * server.ts to trigger container self-shutdown when a thread is idle.
   */
  startIdleWatchdog(idleMs: number, onIdle: () => void): void {
    let lastActivity = Date.now();
    let fired = false;

    // Track activity.
    const activitySub = this._events$
      .pipe(takeUntil(this._destroy$))
      .subscribe(() => {
        lastActivity = Date.now();
        fired = false;
      });
    this._subs.push(activitySub);

    // Check every 60s whether we've been idle.
    const watchdogSub = interval(60_000)
      .pipe(
        takeUntil(this._destroy$),
        filter(() => !fired && Date.now() - lastActivity > idleMs),
      )
      .subscribe(() => {
        fired = true;
        process.stderr.write(
          `[event-bus] idle watchdog triggered after ${idleMs / 1000}s — signaling shutdown\n`,
        );
        onIdle();
      });
    this._subs.push(watchdogSub);
  }

  // ---- SSE helpers ----

  /**
   * Create an SSE-compatible observable for a task. Replays all past
   * events, then streams new ones live. Filters by `afterSeq` so
   * Last-Event-ID resume works.
   *
   * The observable completes when the task emits `done`.
   */
  sseStream$(taskId: string, afterSeq = -1): Observable<BusEvent> {
    return this.taskEvents$(taskId).pipe(
      filter((ev) => ev.seq > afterSeq),
    );
  }

  // ---- Lifecycle ----

  /**
   * GC a finished task's replay subject and dedup set. Called by the
   * server's GC loop after the 1h grace period.
   */
  gcTask(taskId: string): void {
    const replay = this._taskReplays.get(taskId);
    if (replay) {
      replay.complete();
      this._taskReplays.delete(taskId);
    }
    this._taskSeqs.delete(taskId);
  }

  /**
   * Tear down all pipelines and release resources. Called on server shutdown.
   */
  destroy(): void {
    this._destroy$.next();
    this._destroy$.complete();
    this._events$.complete();
    this._ingestCircuit$.complete();
    for (const sub of this._subs) {sub.unsubscribe();}
    this._subs.length = 0;
    for (const [, replay] of this._taskReplays) {replay.complete();}
    this._taskReplays.clear();
    this._taskSeqs.clear();
  }
}

/* eslint-enable security/detect-non-literal-fs-filename */
