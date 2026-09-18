import assert from 'node:assert/strict';
import test from 'node:test';

import {
  OriginPoliteness,
  OriginPolitenessError,
  computeBackoffMs,
  parseRetryAfterMs,
  type OriginPolitenessOptions,
} from '../src/origin-politeness.js';

const ORIGIN = 'https://example.test';

/** Deterministic clock whose sleep advances time instead of waiting. */
function fakeClock(start = 1_000_000) {
  let current = start;
  const sleeps: number[] = [];
  return {
    now: () => current,
    advance: (ms: number) => {
      current += ms;
    },
    sleep: async (ms: number) => {
      sleeps.push(ms);
      current += ms;
    },
    sleeps,
  };
}

function controller(overrides: Partial<OriginPolitenessOptions> = {}, clock = fakeClock()) {
  const politeness = new OriginPoliteness({
    maxPerOrigin: 1,
    maxWaitMs: 30_000,
    baseBackoffMs: 2_000,
    maxBackoffMs: 300_000,
    now: clock.now,
    sleep: clock.sleep,
    random: () => 1,
    ...overrides,
  });
  return { politeness, clock };
}

test('computeBackoffMs grows exponentially, is capped, and keeps a jitter floor', () => {
  assert.equal(computeBackoffMs(0, 1_000, 60_000, () => 0), 0);
  assert.equal(computeBackoffMs(1, 1_000, 60_000, () => 1), 1_000);
  assert.equal(computeBackoffMs(1, 1_000, 60_000, () => 0), 500);
  assert.equal(computeBackoffMs(3, 1_000, 60_000, () => 1), 4_000);
  assert.equal(computeBackoffMs(50, 1_000, 60_000, () => 1), 60_000);
  assert.equal(computeBackoffMs(50, 1_000, 60_000, () => 0), 30_000);
  // out-of-range randomness is clamped rather than exceeding the cap
  assert.equal(computeBackoffMs(1, 1_000, 60_000, () => 7), 1_000);
});

test('parseRetryAfterMs accepts delta-seconds and HTTP-dates and rejects junk', () => {
  const now = Date.parse('Sat, 12 Sep 2026 10:00:00 GMT');
  assert.equal(parseRetryAfterMs('120', now), 120_000);
  assert.equal(parseRetryAfterMs(' 0 ', now), 0);
  assert.equal(parseRetryAfterMs('Sat, 12 Sep 2026 10:00:30 GMT', now), 30_000);
  assert.equal(parseRetryAfterMs('Sat, 12 Sep 2026 09:00:00 GMT', now), 0);
  assert.equal(parseRetryAfterMs(undefined, now), undefined);
  assert.equal(parseRetryAfterMs(null, now), undefined);
  assert.equal(parseRetryAfterMs('', now), undefined);
  assert.equal(parseRetryAfterMs('-5', now), undefined);
  assert.equal(parseRetryAfterMs('1.5', now), undefined);
  assert.equal(parseRetryAfterMs('soon', now), undefined);
  assert.equal(parseRetryAfterMs('2026', now), 2_026_000);
});

test('constructor fails closed on invalid limits', () => {
  assert.throws(() => controller({ maxPerOrigin: 0 }), RangeError);
  assert.throws(() => controller({ maxPerOrigin: 1.5 }), RangeError);
  assert.throws(() => controller({ maxWaitMs: -1 }), RangeError);
  assert.throws(() => controller({ maxBackoffMs: Number.NaN }), RangeError);
  assert.throws(() => controller({ maxTrackedOrigins: 0 }), RangeError);
});

test('spaces request starts to one origin by the crawl delay', async () => {
  const { politeness, clock } = controller({ maxPerOrigin: 4 });
  const releaseA = await politeness.acquire(ORIGIN, 1_000, 30_000);
  const releaseB = await politeness.acquire(ORIGIN, 1_000, 30_000);
  const releaseC = await politeness.acquire('https://other.test', 1_000, 30_000);
  assert.deepEqual(clock.sleeps, [1_000]);
  releaseA();
  releaseB();
  releaseC();
});

test('rejects instead of sleeping past the caller budget', async () => {
  const { politeness, clock } = controller({ maxPerOrigin: 4 });
  (await politeness.acquire(ORIGIN, 10_000, 30_000))();
  await assert.rejects(politeness.acquire(ORIGIN, 10_000, 5_000), (error: unknown) => {
    assert.ok(error instanceof OriginPolitenessError);
    assert.equal(error.code, 'origin-politeness-deferred');
    assert.equal(error.retryAfterMs, 10_000);
    assert.match(error.message, /crawl delay exceeds the wait budget/);
    return true;
  });
  assert.deepEqual(clock.sleeps, []);
  assert.equal(politeness.counters.rejections, 1);
  // the rejected caller must not have consumed a slot or pushed the schedule
  assert.equal(politeness.snapshot().activeRequests, 0);
  clock.advance(10_000);
  const release = await politeness.acquire(ORIGIN, 10_000, 5_000);
  assert.deepEqual(clock.sleeps, []);
  release();
});

test('maxWaitMs bounds the wait even when the caller budget is larger', async () => {
  const { politeness } = controller({ maxPerOrigin: 4, maxWaitMs: 2_000 });
  (await politeness.acquire(ORIGIN, 5_000, 60_000))();
  await assert.rejects(politeness.acquire(ORIGIN, 5_000, 60_000), OriginPolitenessError);
});

test('caps concurrent requests per origin and hands the slot to a waiter on release', async () => {
  const { politeness } = controller({ maxPerOrigin: 1 }, {
    ...fakeClock(),
    // real-time clock semantics are not needed; spacing is 0 in this test
  });
  const release = await politeness.acquire(ORIGIN, 0, 30_000);
  let secondAcquired = false;
  const second = politeness.acquire(ORIGIN, 0, 30_000).then((releaseSecond) => {
    secondAcquired = true;
    return releaseSecond;
  });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(secondAcquired, false);
  assert.equal(politeness.snapshot().waitingRequests, 1);
  release();
  release(); // idempotent: must not free a second slot
  const releaseSecond = await second;
  assert.equal(secondAcquired, true);
  assert.equal(politeness.snapshot().activeRequests, 1);
  releaseSecond();
  assert.equal(politeness.snapshot().activeRequests, 0);
});

test('a saturated origin rejects a waiter once its real wait bound elapses', async () => {
  const politeness = new OriginPoliteness({
    maxPerOrigin: 1,
    maxWaitMs: 25,
    baseBackoffMs: 1_000,
    maxBackoffMs: 10_000,
  });
  const release = await politeness.acquire(ORIGIN, 0, 1_000);
  await assert.rejects(politeness.acquire(ORIGIN, 0, 1_000), (error: unknown) => {
    assert.ok(error instanceof OriginPolitenessError);
    assert.match(error.message, /per-origin concurrency limit \(1\) reached/);
    assert.ok(error.retryAfterMs >= 1_000);
    return true;
  });
  // the timed-out waiter was removed, so release does not wake a ghost
  assert.equal(politeness.snapshot().waitingRequests, 0);
  release();
  (await politeness.acquire(ORIGIN, 0, 1_000))();
});

test('429 with Retry-After defers the origin and rejects callers that cannot wait', async () => {
  const { politeness, clock } = controller({ maxPerOrigin: 2 });
  const release = await politeness.acquire(ORIGIN, 0, 30_000);
  const applied = politeness.recordResponse(ORIGIN, 429, '60');
  release();
  assert.equal(applied, 60_000);
  assert.equal(politeness.counters.backoffs, 1);
  assert.equal(politeness.snapshot().originsInBackoff, 1);

  await assert.rejects(politeness.acquire(ORIGIN, 0, 30_000), (error: unknown) => {
    assert.ok(error instanceof OriginPolitenessError);
    assert.match(error.message, /asked us to slow down/);
    assert.equal(error.retryAfterMs, 60_000);
    return true;
  });

  // Other origins are unaffected.
  (await politeness.acquire('https://other.test', 0, 30_000))();

  clock.advance(60_000);
  assert.equal(politeness.snapshot().originsInBackoff, 0);
  (await politeness.acquire(ORIGIN, 0, 30_000))();
});

test('a caller within budget waits out a short back-off instead of failing', async () => {
  const { politeness, clock } = controller();
  politeness.recordResponse(ORIGIN, 503, '3');
  const release = await politeness.acquire(ORIGIN, 0, 30_000);
  assert.deepEqual(clock.sleeps, [3_000]);
  release();
});

test('Retry-After is capped by maxBackoffMs', () => {
  const { politeness } = controller({ maxBackoffMs: 120_000 });
  assert.equal(politeness.recordResponse(ORIGIN, 429, '86400'), 120_000);
});

test('repeated throttles back off exponentially and a success resets the streak', () => {
  const { politeness } = controller({ baseBackoffMs: 1_000, maxBackoffMs: 60_000 });
  assert.equal(politeness.recordResponse(ORIGIN, 429), 1_000);
  assert.equal(politeness.recordResponse(ORIGIN, 429), 2_000);
  assert.equal(politeness.recordResponse(ORIGIN, 503), 4_000);
  assert.equal(politeness.recordResponse(ORIGIN, 200), 0);
  assert.equal(politeness.recordResponse(ORIGIN, 429), 1_000);
  // 5xx other than 503 neither backs off nor resets the streak
  assert.equal(politeness.recordResponse(ORIGIN, 500), 0);
  assert.equal(politeness.recordResponse(ORIGIN, 429), 2_000);
  // unknown status is ignored
  assert.equal(politeness.recordResponse(ORIGIN, undefined), 0);
});

test('a back-off window only ever extends, never shrinks', async () => {
  const { politeness } = controller({ baseBackoffMs: 1_000, maxBackoffMs: 300_000 });
  politeness.recordResponse(ORIGIN, 429, '120');
  politeness.recordResponse(ORIGIN, 429, '1');
  await assert.rejects(politeness.acquire(ORIGIN, 0, 30_000), (error: unknown) => {
    assert.ok(error instanceof OriginPolitenessError);
    assert.equal(error.retryAfterMs, 120_000);
    return true;
  });
});

test('eviction never forgets an origin that is still owed a back-off', async () => {
  const { politeness, clock } = controller({ maxTrackedOrigins: 2, maxPerOrigin: 4 });
  politeness.recordResponse(ORIGIN, 429, '600');
  (await politeness.acquire('https://a.test', 0, 30_000))();
  (await politeness.acquire('https://b.test', 0, 30_000))();
  (await politeness.acquire('https://c.test', 0, 30_000))();
  await assert.rejects(politeness.acquire(ORIGIN, 0, 30_000), OriginPolitenessError);
  // idle origins were evicted to stay near the cap
  assert.ok(politeness.snapshot().trackedOrigins <= 3);
  clock.advance(600_000);
  (await politeness.acquire(ORIGIN, 0, 30_000))();
});

test('a failing sleep releases the reserved slot', async () => {
  const clock = fakeClock();
  const politeness = new OriginPoliteness({
    maxPerOrigin: 1,
    maxWaitMs: 30_000,
    baseBackoffMs: 1_000,
    maxBackoffMs: 10_000,
    now: clock.now,
    sleep: async () => {
      throw new Error('aborted');
    },
  });
  (await politeness.acquire(ORIGIN, 1_000, 30_000))();
  await assert.rejects(politeness.acquire(ORIGIN, 1_000, 30_000), /aborted/);
  assert.equal(politeness.snapshot().activeRequests, 0);
});
