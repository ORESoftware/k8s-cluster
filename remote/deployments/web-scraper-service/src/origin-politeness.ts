/**
 * Per-origin politeness controller for the local scraper.
 *
 * Modelled on the "be a polite visitor" controls mature crawlers (Apify
 * Crawlee's per-domain concurrency / session back-off) apply, but kept
 * deliberately small and in-process:
 *
 * - a hard cap on concurrent requests to one origin (`maxPerOrigin`);
 * - minimum spacing between request starts to one origin (the robots.txt
 *   Crawl-delay or `SCRAPER_MIN_ORIGIN_DELAY_MS`, supplied per call);
 * - adaptive back-off when the target answers 429/503, honouring a bounded
 *   `Retry-After` and otherwise using capped exponential back-off with jitter;
 * - a bounded wait: a caller is never parked longer than `maxWaitMs` (or its
 *   own budget). If the origin cannot be visited in time the caller gets an
 *   {@link OriginPolitenessError} carrying a `retryAfterMs` hint instead of
 *   silently holding an in-flight slot.
 *
 * This is a courtesy/rate-limit mechanism only. It never rotates identity,
 * never retries against a target that asked us to slow down, and a politeness
 * rejection must not be escalated to a remote provider (the HTTP layer maps it
 * to 429, which the supervisor never treats as fallback-eligible).
 */

export type OriginPolitenessOptions = {
  /** Maximum concurrent requests to a single origin (>= 1). */
  maxPerOrigin: number;
  /** Maximum time a caller may wait for its turn at an origin. */
  maxWaitMs: number;
  /** First back-off step after a 429/503 when no Retry-After is supplied. */
  baseBackoffMs: number;
  /** Upper bound for any back-off, including a server-supplied Retry-After. */
  maxBackoffMs: number;
  /** Soft cap on tracked origins; idle origins are evicted first. */
  maxTrackedOrigins?: number;
  now?: () => number;
  random?: () => number;
  sleep?: (ms: number) => Promise<void>;
};

export type OriginSnapshot = {
  trackedOrigins: number;
  activeRequests: number;
  waitingRequests: number;
  originsInBackoff: number;
};

export type OriginPolitenessCounters = {
  rejections: number;
  backoffs: number;
};

type OriginState = {
  active: number;
  nextRequestAt: number;
  backoffUntil: number;
  consecutiveThrottles: number;
  waiters: Array<() => void>;
};

export class OriginPolitenessError extends Error {
  readonly code = 'origin-politeness-deferred';
  readonly retryAfterMs: number;
  readonly origin: string;

  constructor(origin: string, retryAfterMs: number, reason: string) {
    super(
      `scraper politeness deferred request to ${origin}: ${reason}; retry after ${Math.ceil(retryAfterMs)}ms`,
    );
    this.name = 'OriginPolitenessError';
    this.origin = origin;
    this.retryAfterMs = Math.max(0, Math.ceil(retryAfterMs));
  }
}

const THROTTLE_STATUSES = new Set([429, 503]);

export class OriginPoliteness {
  private readonly origins = new Map<string, OriginState>();
  private readonly options: Required<OriginPolitenessOptions>;
  readonly counters: OriginPolitenessCounters = { rejections: 0, backoffs: 0 };

  constructor(options: OriginPolitenessOptions) {
    assertPositiveInteger('maxPerOrigin', options.maxPerOrigin);
    assertNonNegativeFinite('maxWaitMs', options.maxWaitMs);
    assertNonNegativeFinite('baseBackoffMs', options.baseBackoffMs);
    assertNonNegativeFinite('maxBackoffMs', options.maxBackoffMs);
    this.options = {
      maxTrackedOrigins: 1_024,
      now: Date.now,
      random: Math.random,
      sleep: defaultSleep,
      ...options,
    };
    assertPositiveInteger('maxTrackedOrigins', this.options.maxTrackedOrigins);
  }

  /**
   * Wait for a polite turn at `origin`. Resolves with an idempotent release
   * function that MUST be called (in a `finally`) once the request to the
   * origin is finished.
   *
   * @param delayMs minimum spacing to reserve after this request's start.
   * @param budgetMs the caller's own remaining budget; the effective wait bound
   *   is `min(maxWaitMs, budgetMs)`.
   */
  async acquire(origin: string, delayMs: number, budgetMs: number): Promise<() => void> {
    const { now } = this.options;
    const spacingMs = Math.max(0, Number.isFinite(delayMs) ? delayMs : 0);
    const waitBoundMs = Math.max(
      0,
      Math.min(this.options.maxWaitMs, Number.isFinite(budgetMs) ? budgetMs : 0),
    );
    const deadline = now() + waitBoundMs;
    const state = this.stateFor(origin);

    while (state.active >= this.options.maxPerOrigin) {
      const remaining = deadline - now();
      if (remaining <= 0 || !(await waitForSlot(state, remaining))) {
        this.counters.rejections += 1;
        // Best hint available: the earliest spacing/back-off boundary, or a
        // conservative one-second retry when the origin is simply saturated.
        const hint = Math.max(1_000, state.backoffUntil - now(), state.nextRequestAt - now());
        throw new OriginPolitenessError(
          origin,
          hint,
          `per-origin concurrency limit (${this.options.maxPerOrigin}) reached`,
        );
      }
    }

    const current = now();
    const scheduledAt = Math.max(current, state.nextRequestAt, state.backoffUntil);
    if (scheduledAt > deadline) {
      this.counters.rejections += 1;
      const inBackoff = state.backoffUntil > current && state.backoffUntil >= state.nextRequestAt;
      throw new OriginPolitenessError(
        origin,
        scheduledAt - current,
        inBackoff
          ? 'origin asked us to slow down (429/503 back-off active)'
          : 'origin crawl delay exceeds the wait budget',
      );
    }

    state.active += 1;
    state.nextRequestAt = scheduledAt + spacingMs;
    let released = false;
    const release = (): void => {
      if (released) return;
      released = true;
      state.active = Math.max(0, state.active - 1);
      const next = state.waiters.shift();
      if (next) next();
    };

    if (scheduledAt > current) {
      try {
        await this.options.sleep(scheduledAt - current);
      } catch (error) {
        release();
        throw error;
      }
    }
    return release;
  }

  /**
   * Feed the origin's HTTP status back into the controller. 429/503 extend the
   * origin's back-off window; any other definite non-5xx response resets the
   * consecutive-throttle counter (an existing window still runs out naturally).
   * Returns the back-off applied in milliseconds (0 when none).
   */
  recordResponse(origin: string, status: number | undefined, retryAfter?: string | null): number {
    if (status === undefined || !Number.isFinite(status)) return 0;
    const state = this.stateFor(origin);
    if (!THROTTLE_STATUSES.has(status)) {
      if (status < 500) state.consecutiveThrottles = 0;
      return 0;
    }

    state.consecutiveThrottles += 1;
    const current = this.options.now();
    const exponential = computeBackoffMs(
      state.consecutiveThrottles,
      this.options.baseBackoffMs,
      this.options.maxBackoffMs,
      this.options.random,
    );
    const serverHint = parseRetryAfterMs(retryAfter, current);
    const backoffMs = Math.min(
      this.options.maxBackoffMs,
      Math.max(exponential, serverHint ?? 0),
    );
    state.backoffUntil = Math.max(state.backoffUntil, current + backoffMs);
    this.counters.backoffs += 1;
    return backoffMs;
  }

  snapshot(): OriginSnapshot {
    const current = this.options.now();
    let activeRequests = 0;
    let waitingRequests = 0;
    let originsInBackoff = 0;
    for (const state of this.origins.values()) {
      activeRequests += state.active;
      waitingRequests += state.waiters.length;
      if (state.backoffUntil > current) originsInBackoff += 1;
    }
    return {
      trackedOrigins: this.origins.size,
      activeRequests,
      waitingRequests,
      originsInBackoff,
    };
  }

  private stateFor(origin: string): OriginState {
    const existing = this.origins.get(origin);
    if (existing) {
      // Refresh insertion order so eviction prefers least-recently-used origins.
      this.origins.delete(origin);
      this.origins.set(origin, existing);
      return existing;
    }
    this.evictIdle();
    const created: OriginState = {
      active: 0,
      nextRequestAt: 0,
      backoffUntil: 0,
      consecutiveThrottles: 0,
      waiters: [],
    };
    this.origins.set(origin, created);
    return created;
  }

  /**
   * Evict least-recently-used origins that hold no politeness obligation (no
   * active/waiting requests, no pending spacing or back-off). Origins that are
   * still owed courtesy are never forgotten, so eviction cannot be used to
   * reset a target's back-off; the map is then bounded by in-flight work.
   */
  private evictIdle(): void {
    if (this.origins.size < this.options.maxTrackedOrigins) return;
    const current = this.options.now();
    for (const [key, state] of this.origins) {
      if (this.origins.size < this.options.maxTrackedOrigins) return;
      if (
        state.active === 0 &&
        state.waiters.length === 0 &&
        state.nextRequestAt <= current &&
        state.backoffUntil <= current
      ) {
        this.origins.delete(key);
      }
    }
  }
}

/**
 * Capped exponential back-off with "equal jitter": half the step is fixed and
 * half is random, so retries spread out without ever collapsing to ~0.
 */
export function computeBackoffMs(
  attempt: number,
  baseMs: number,
  maxMs: number,
  random: () => number = Math.random,
): number {
  if (!Number.isFinite(attempt) || attempt < 1 || baseMs <= 0 || maxMs <= 0) return 0;
  const exponent = Math.min(30, Math.floor(attempt) - 1);
  const step = Math.min(maxMs, baseMs * 2 ** exponent);
  const jitter = Math.min(1, Math.max(0, random()));
  return Math.min(maxMs, Math.round(step / 2 + (step / 2) * jitter));
}

/**
 * Parse an HTTP `Retry-After` header (delta-seconds or HTTP-date) into
 * milliseconds from `nowMs`. Returns undefined for absent/invalid values and
 * never returns a negative delay.
 */
export function parseRetryAfterMs(value: string | null | undefined, nowMs: number): number | undefined {
  if (value === null || value === undefined) return undefined;
  const trimmed = value.trim();
  if (trimmed === '') return undefined;
  if (/^\d+$/.test(trimmed)) {
    const seconds = Number(trimmed);
    return Number.isFinite(seconds) ? seconds * 1_000 : undefined;
  }
  // Only accept HTTP-date shaped values; Date.parse is too permissive otherwise.
  if (!/[a-z]{3},?\s/i.test(trimmed) && !/GMT$/i.test(trimmed)) return undefined;
  const at = Date.parse(trimmed);
  if (!Number.isFinite(at)) return undefined;
  return Math.max(0, at - nowMs);
}

function waitForSlot(state: OriginState, timeoutMs: number): Promise<boolean> {
  return new Promise((resolve) => {
    let settled = false;
    const wake = (): void => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve(true);
    };
    const timer = setTimeout(() => {
      if (settled) return;
      settled = true;
      const index = state.waiters.indexOf(wake);
      if (index >= 0) state.waiters.splice(index, 1);
      resolve(false);
    }, timeoutMs);
    // Deliberately NOT unref'd: a request waiting for its turn is live work.
    state.waiters.push(wake);
  });
}

function defaultSleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function assertPositiveInteger(name: string, value: number): void {
  if (!Number.isInteger(value) || value < 1) {
    throw new RangeError(`${name} must be a positive integer`);
  }
}

function assertNonNegativeFinite(name: string, value: number): void {
  if (!Number.isFinite(value) || value < 0) {
    throw new RangeError(`${name} must be a non-negative finite number`);
  }
}
