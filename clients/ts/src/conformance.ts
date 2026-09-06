// Behavioral conformance cross-check between the two libraries:
//
//   ours:   rust-network-mutex-rs broker   (camelCase enum protocol)
//   theirs: oresoftware/live-mutex broker   (legacy kebab-case protocol)
//
// Unlike `compare.ts` (which only measures throughput), this runs the *same*
// behavioral scenarios against both brokers through a uniform adapter and
// asserts the lock semantics match. It prints a per-scenario table and a final
// verdict, and exits non-zero if a *correctness* invariant (mutual exclusion,
// no lost updates) is violated on either broker.
//
// Feature gaps (fencing tokens, composite locks) are reported as DIVERGENCE
// rather than failure — they are real, expected differences between the two
// implementations and are part of the audit output.
//
//   OURS_HOST=127.0.0.1 OURS_PORT=6970 \
//   THEIRS_HOST=127.0.0.1 THEIRS_PORT=7970 \
//   WORKERS=8 ITERS=50 DURATION_MS=2000 \
//   LIVE_MUTEX_PKG=live-mutex \
//     tsx src/conformance.ts

import { performance } from "node:perf_hooks";
import { NetworkMutexClient } from "./client.ts";
import { Broker1Client } from "./lmx-broker1-client.ts";

const OURS_HOST = process.env["OURS_HOST"] ?? "127.0.0.1";
const OURS_PORT = Number(process.env["OURS_PORT"] ?? 6970);
const THEIRS_HOST = process.env["THEIRS_HOST"] ?? "127.0.0.1";
const THEIRS_PORT = Number(process.env["THEIRS_PORT"] ?? 7970);
const LIVE_MUTEX_PKG = process.env["LIVE_MUTEX_PKG"] ?? "live-mutex";

const WORKERS = Number(process.env["WORKERS"] ?? 8);
const ITERS = Number(process.env["ITERS"] ?? 50);
const DURATION_MS = Number(process.env["DURATION_MS"] ?? 1500);
const KEYS = Number(process.env["KEYS"] ?? 4);

// ---- uniform adapter ------------------------------------------------------

interface Handle {
  key: string;
  fencing: number | null;
}

interface CompositeHandle {
  keys: string[];
  fencingTokens: Record<string, number>;
}

interface Session {
  acquire(key: string): Promise<Handle>;
  release(h: Handle): Promise<void>;
  acquireMany?(keys: string[]): Promise<CompositeHandle>;
  releaseMany?(h: CompositeHandle): Promise<void>;
  close(): Promise<void>;
}

interface Backend {
  name: string;
  supportsFencing: boolean;
  supportsComposite: boolean;
  newSession(): Promise<Session>;
  shutdown(): Promise<void>;
}

async function makeOurs(): Promise<Backend> {
  const sessions: NetworkMutexClient[] = [];
  return {
    name: "ours (rust)",
    supportsFencing: true,
    supportsComposite: true,
    async newSession(): Promise<Session> {
      const client = new NetworkMutexClient({ host: OURS_HOST, port: OURS_PORT });
      await client.connect();
      sessions.push(client);
      return {
        async acquire(key: string): Promise<Handle> {
          const h = await client.acquire(key, { ttlMs: 30_000, waitMs: 30_000 });
          return { key, fencing: h.fencingToken, _raw: h } as Handle & { _raw: unknown };
        },
        async release(h: Handle): Promise<void> {
          await client.release((h as Handle & { _raw: unknown })._raw as never);
        },
        async acquireMany(keys: string[]): Promise<CompositeHandle> {
          // Blocking acquire-many: with caller-controlled wait now supported on
          // both brokers (and the composite-queue client bug fixed), this
          // blocks until every key is granted — no client-side retry needed.
          const h = await client.acquireMany(keys, { ttlMs: 30_000, waitMs: 30_000 });
          return { keys: h.keys, fencingTokens: h.fencingTokens, _raw: h } as CompositeHandle & { _raw: unknown };
        },
        async releaseMany(h: CompositeHandle): Promise<void> {
          await client.release((h as CompositeHandle & { _raw: unknown })._raw as never);
        },
        async close(): Promise<void> {
          await client.close();
        },
      };
    },
    async shutdown(): Promise<void> {
      await Promise.allSettled(sessions.map((s) => s.close()));
    },
  };
}

// "theirs" = oresoftware/live-mutex `Broker1` (fencing-token-aware). We speak
// its wire protocol directly (see lmx-broker1-client.ts) so the cross-check
// compares both brokers on equal footing — including fencing tokens and
// composite (acquire-many) holds, which the legacy npm client never surfaces.
async function makeTheirs(): Promise<Backend | null> {
  const sessions: Broker1Client[] = [];
  // Probe one connection up front so an unavailable broker is skipped cleanly.
  try {
    const probe = new Broker1Client(THEIRS_HOST, THEIRS_PORT);
    await probe.connect();
    await probe.close();
  } catch (err) {
    console.error(`[conformance] Broker1 at ${THEIRS_HOST}:${THEIRS_PORT} unavailable, skipping THEIRS:`, (err as Error).message);
    return null;
  }
  return {
    name: "theirs (ts Broker1)",
    supportsFencing: true,
    supportsComposite: true,
    async newSession(): Promise<Session> {
      const client = new Broker1Client(THEIRS_HOST, THEIRS_PORT);
      await client.connect();
      sessions.push(client);
      return {
        async acquire(key: string): Promise<Handle> {
          const h = await client.acquire(key, 30_000);
          return { key, fencing: h.fencingToken, _raw: h } as Handle & { _raw: unknown };
        },
        async release(h: Handle): Promise<void> {
          await client.release((h as Handle & { _raw: unknown })._raw as never);
        },
        async acquireMany(keys: string[]): Promise<CompositeHandle> {
          const h = await client.acquireMany(keys, 30_000);
          return { keys: h.keys, fencingTokens: h.fencingTokens, _raw: h } as CompositeHandle & { _raw: unknown };
        },
        async releaseMany(h: CompositeHandle): Promise<void> {
          await client.releaseMany((h as CompositeHandle & { _raw: unknown })._raw as never);
        },
        async close(): Promise<void> {
          await client.close();
        },
      };
    },
    async shutdown(): Promise<void> {
      await Promise.allSettled(sessions.map((s) => s.close()));
    },
  };
}

// ---- scenarios ------------------------------------------------------------

interface ScenarioResult {
  scenario: string;
  ok: boolean;
  detail: string;
}

const tick = (): Promise<void> => new Promise((r) => setImmediate(r));

/**
 * Mutual exclusion: WORKERS sessions each run ITERS acquire/critical/release
 * loops on a single hot key. A shared in-process counter must never exceed 1
 * while the lock is held, and the total number of completed critical sections
 * must equal WORKERS*ITERS (no lost updates / no dropped grants). This is the
 * core correctness property both brokers must uphold.
 */
async function mutualExclusion(backend: Backend): Promise<ScenarioResult> {
  const key = `conf-mutex-${Date.now()}`;
  let active = 0;
  let maxConcurrent = 0;
  let completed = 0;
  let violations = 0;

  const sessions = await Promise.all(Array.from({ length: WORKERS }, () => backend.newSession()));
  await Promise.all(
    sessions.map(async (s) => {
      for (let i = 0; i < ITERS; i++) {
        const h = await s.acquire(key);
        active += 1;
        if (active > maxConcurrent) maxConcurrent = active;
        if (active > 1) violations += 1;
        await tick(); // yield so a broken broker could interleave here
        active -= 1;
        completed += 1;
        await s.release(h);
      }
    }),
  );
  await Promise.allSettled(sessions.map((s) => s.close()));

  const expected = WORKERS * ITERS;
  const ok = maxConcurrent === 1 && violations === 0 && completed === expected;
  return {
    scenario: "mutual-exclusion",
    ok,
    detail: `maxConcurrent=${maxConcurrent} violations=${violations} completed=${completed}/${expected}`,
  };
}

/**
 * Fencing-token monotonicity: sequential acquire/release on one key must yield
 * strictly increasing tokens. Only meaningful when the backend advertises
 * fencing support; otherwise reported as a divergence.
 */
async function fencingMonotonic(backend: Backend): Promise<ScenarioResult> {
  if (!backend.supportsFencing) {
    return { scenario: "fencing-monotonic", ok: true, detail: "DIVERGENCE: backend has no fencing tokens (skipped)" };
  }
  const key = `conf-fence-${Date.now()}`;
  const s = await backend.newSession();
  let last = -1;
  let ok = true;
  for (let i = 0; i < 16; i++) {
    const h = await s.acquire(key);
    const t = h.fencing ?? -1;
    if (!(t > last)) ok = false;
    last = t;
    await s.release(h);
  }
  await s.close();
  return { scenario: "fencing-monotonic", ok, detail: `lastToken=${last}` };
}

/**
 * Composite (acquire-many) atomicity + deadlock freedom. WORKERS sessions each
 * acquire the SAME set of keys (each worker submits them in a different order
 * to stress the broker's deadlock-avoidance ordering) and release. Because the
 * key sets fully overlap, at most one composite hold may be active at a time —
 * so this is mutual exclusion at the multi-key granularity. Every grant must
 * also carry a fencing token per key. Reported as DIVERGENCE for backends
 * without composite support.
 */
async function compositeAtomic(backend: Backend): Promise<ScenarioResult> {
  if (!backend.supportsComposite) {
    return { scenario: "composite-atomic", ok: true, detail: "DIVERGENCE: backend has no acquire-many (skipped)" };
  }
  const base = `conf-comp-${Date.now()}`;
  const keys = [`${base}-x`, `${base}-y`, `${base}-z`];
  const iters = Math.min(ITERS, 20);
  let active = 0;
  let maxConcurrent = 0;
  let violations = 0;
  let completed = 0;
  let missingTokens = 0;

  const sessions = await Promise.all(Array.from({ length: WORKERS }, () => backend.newSession()));
  await Promise.all(
    sessions.map(async (s, idx) => {
      if (!s.acquireMany || !s.releaseMany) return;
      // Each worker submits the keys in a rotated order to exercise the
      // broker's sorted-acquisition deadlock avoidance.
      const order = keys.slice(idx % keys.length).concat(keys.slice(0, idx % keys.length));
      for (let i = 0; i < iters; i++) {
        const h = await s.acquireMany(order);
        active += 1;
        if (active > maxConcurrent) maxConcurrent = active;
        if (active > 1) violations += 1;
        if (keys.some((k) => !(k in h.fencingTokens) || !(h.fencingTokens[k]! > 0))) missingTokens += 1;
        await tick();
        active -= 1;
        completed += 1;
        await s.releaseMany(h);
      }
    }),
  );
  await Promise.allSettled(sessions.map((s) => s.close()));

  const expected = WORKERS * iters;
  const ok = maxConcurrent === 1 && violations === 0 && completed === expected && missingTokens === 0;
  return {
    scenario: "composite-atomic",
    ok,
    detail: `maxConcurrent=${maxConcurrent} violations=${violations} missingTokens=${missingTokens} completed=${completed}/${expected}`,
  };
}

interface ThroughputResult extends ScenarioResult {
  ops: number;
  throughput: number;
  avgMs: number;
  errors: number;
}

async function throughput(backend: Backend): Promise<ThroughputResult> {
  const deadline = performance.now() + DURATION_MS;
  let ops = 0;
  let errors = 0;
  let totalNs = 0n;

  const sessions = await Promise.all(Array.from({ length: WORKERS }, () => backend.newSession()));
  await Promise.all(
    sessions.map(async (s) => {
      while (performance.now() < deadline) {
        const key = `conf-thr-${Math.floor(Math.random() * KEYS)}`;
        const t0 = process.hrtime.bigint();
        try {
          const h = await s.acquire(key);
          await s.release(h);
          totalNs += process.hrtime.bigint() - t0;
          ops += 1;
        } catch {
          errors += 1;
        }
      }
    }),
  );
  await Promise.allSettled(sessions.map((s) => s.close()));

  const throughputOps = ops / (DURATION_MS / 1000);
  const avgMs = ops === 0 ? 0 : Number(totalNs / BigInt(ops)) / 1e6;
  return {
    scenario: "throughput",
    ok: errors === 0,
    detail: `ops=${ops} thr=${throughputOps.toFixed(0)}/s avg=${avgMs.toFixed(2)}ms errors=${errors}`,
    ops,
    throughput: throughputOps,
    avgMs,
    errors,
  };
}

async function runBackend(backend: Backend): Promise<{ results: ScenarioResult[]; thr: ThroughputResult }> {
  console.log(`\n[conformance] === ${backend.name} ===`);
  const results: ScenarioResult[] = [];
  results.push(await mutualExclusion(backend));
  results.push(await fencingMonotonic(backend));
  results.push(await compositeAtomic(backend));
  const thr = await throughput(backend);
  results.push(thr);
  for (const r of results) {
    console.log(`  ${r.ok ? "PASS" : "FAIL"}  ${r.scenario.padEnd(18)} ${r.detail}`);
  }
  await backend.shutdown();
  return { results, thr };
}

async function main(): Promise<void> {
  console.log(
    `[conformance] workers=${WORKERS} iters=${ITERS} duration=${DURATION_MS}ms keys=${KEYS}\n` +
      `[conformance] ours=${OURS_HOST}:${OURS_PORT} theirs=${THEIRS_HOST}:${THEIRS_PORT} pkg=${LIVE_MUTEX_PKG}`,
  );

  const ours = await makeOurs();
  const oursOut = await runBackend(ours);

  const theirs = await makeTheirs();
  const theirsOut = theirs ? await runBackend(theirs) : null;

  // Correctness gate: mutual exclusion + no lost updates must hold on every
  // available backend.
  const correctnessScenarios = ["mutual-exclusion", "fencing-monotonic", "composite-atomic"];
  const failed: string[] = [];
  for (const r of oursOut.results) {
    if (correctnessScenarios.includes(r.scenario) && !r.ok) failed.push(`ours/${r.scenario}`);
  }
  if (theirsOut) {
    for (const r of theirsOut.results) {
      if (correctnessScenarios.includes(r.scenario) && !r.ok) failed.push(`theirs/${r.scenario}`);
    }
  }

  console.log("\n[conformance] ---- summary ----");
  console.log(`  mutual exclusion upheld: ours=${oursOut.results[0]?.ok} theirs=${theirsOut?.results[0]?.ok ?? "n/a"}`);
  console.log(
    `  fencing tokens:          ours=${ours.supportsFencing ? "yes" : "no"} theirs=${
      theirs ? (theirs.supportsFencing ? "yes" : "no (DIVERGENCE)") : "n/a"
    }`,
  );
  console.log(
    `  composite (multi-key):   ours=${ours.supportsComposite ? "yes" : "no"} theirs=${
      theirs ? (theirs.supportsComposite ? "yes" : "no (DIVERGENCE)") : "n/a"
    }`,
  );
  if (theirsOut) {
    const ratio = theirsOut.thr.ops > 0 ? oursOut.thr.ops / theirsOut.thr.ops : 0;
    console.log(
      `  throughput:              ours=${oursOut.thr.throughput.toFixed(0)}/s theirs=${theirsOut.thr.throughput.toFixed(0)}/s ratio=${ratio.toFixed(2)}x`,
    );
  }

  if (failed.length > 0) {
    console.error(`\n[conformance] VERDICT: FAIL — correctness violations: ${failed.join(", ")}`);
    process.exitCode = 1;
  } else if (!theirsOut) {
    console.log("\n[conformance] VERDICT: PARTIAL — ours passed; theirs broker unavailable (not started?).");
  } else {
    console.log(
      "\n[conformance] VERDICT: PASS — both brokers uphold mutual exclusion; behavioral differences are limited to documented feature gaps (fencing tokens, composite locks).",
    );
  }
}

main().catch((err) => {
  console.error("[conformance] FATAL", err);
  process.exitCode = 1;
});
