import assert from 'node:assert/strict';
import test from 'node:test';

import {
  boundedAttemptTimeoutMs,
  buildLocalRetryPlan,
  classifyLocalFailure,
  decideNextLocalRetry,
  isLocalRetryEligible,
  remainingBudgetMs,
} from '../src/supervisor-retry-policy.js';

test('policy, auth, and CAPTCHA failures remain terminal', () => {
  const cases: Array<[number, string]> = [
    [500, 'robots.txt denied this target'],
    [500, 'SSRF policy denied private address'],
    [401, 'unauthorized'],
    [403, 'forbidden'],
    [500, 'captcha challenge detected'],
  ];

  for (const [statusCode, message] of cases) {
    const failureClass = classifyLocalFailure(statusCode, message);
    assert.equal(isLocalRetryEligible(statusCode, failureClass), false, `${statusCode} ${message}`);
  }
});

test('only classified retriable local 5xx failures enter the retry ladder', () => {
  for (const message of [
    'navigation timeout after 30000ms',
    'fetch failed: ECONNRESET',
    'playwright page crashed',
    'parser worker exited unexpectedly',
  ]) {
    const failureClass = classifyLocalFailure(502, message);
    assert.equal(isLocalRetryEligible(502, failureClass), true, message);
  }

  assert.equal(isLocalRetryEligible(429, classifyLocalFailure(429, 'network timeout')), false);
  assert.equal(isLocalRetryEligible(500, classifyLocalFailure(500, 'unclassified failure')), false);
});

test('auto retries the other local Chromium adapters before optional browserless', () => {
  assert.deepEqual(
    buildLocalRetryPlan({
      requestedStrategy: 'auto',
      completedStrategies: ['native-fetch'],
      autoUseBrowserless: true,
      browserlessConfigured: true,
    }),
    ['playwright', 'puppeteer', 'browserless'],
  );

  assert.deepEqual(
    buildLocalRetryPlan({
      requestedStrategy: 'auto',
      completedStrategies: ['playwright'],
      autoUseBrowserless: false,
      browserlessConfigured: true,
    }),
    ['puppeteer'],
  );
});

test('explicit strategies remain exact and are never silently widened', () => {
  for (const requestedStrategy of ['native-fetch', 'playwright', 'puppeteer', 'browserless'] as const) {
    assert.deepEqual(
      buildLocalRetryPlan({
        requestedStrategy,
        completedStrategies: [requestedStrategy],
        autoUseBrowserless: true,
        browserlessConfigured: true,
      }),
      [],
      requestedStrategy,
    );
  }
});

test('retry planning never repeats a completed strategy', () => {
  assert.deepEqual(
    buildLocalRetryPlan({
      requestedStrategy: 'auto',
      completedStrategies: ['playwright', 'browserless'],
      autoUseBrowserless: true,
      browserlessConfigured: true,
    }),
    ['puppeteer'],
  );
});

test('wall-clock retry budget is monotonic and clamps at zero', () => {
  assert.equal(remainingBudgetMs(1_000, 1_000, 120_000), 120_000);
  assert.equal(remainingBudgetMs(1_000, 31_000, 120_000), 90_000);
  assert.equal(remainingBudgetMs(1_000, 121_001, 120_000), 0);
  assert.equal(remainingBudgetMs(2_000, 1_000, 120_000), 120_000);
});

test('each retry attempt is capped by the remaining total budget', () => {
  assert.equal(boundedAttemptTimeoutMs(30_000, 90_000), 30_000);
  assert.equal(boundedAttemptTimeoutMs(30_000, 8_250), 8_250);
  assert.equal(boundedAttemptTimeoutMs(30_000, 500), 500);
  assert.equal(boundedAttemptTimeoutMs(30_000, 499), 0);
});

test('provider attempts can use a stricter minimum timeout without exceeding the same budget', () => {
  assert.equal(boundedAttemptTimeoutMs(60_000, 12_500, 1_000), 12_500);
  assert.equal(boundedAttemptTimeoutMs(60_000, 999, 1_000), 0);
});

test('invalid retry-budget inputs fail closed', () => {
  for (const values of [
    [-1, 10_000, 500],
    [30_000, -1, 500],
    [30_000, 10_000, 0],
    [Number.NaN, 10_000, 500],
    [30_000, Number.POSITIVE_INFINITY, 500],
  ] as const) {
    assert.throws(() => boundedAttemptTimeoutMs(...values), TypeError);
  }
});

test('next retry decision chooses the first uncompleted adapter and caps its timeout', () => {
  assert.deepEqual(
    decideNextLocalRetry({
      requestedStrategy: 'auto',
      completedStrategies: ['native-fetch'],
      browserlessConfigured: true,
      autoUseBrowserless: true,
      lastStatusCode: 502,
      lastError: 'fetch failed: ECONNRESET',
      startedAtMs: 1_000,
      nowMs: 116_000,
      maxTotalMs: 120_000,
      requestedTimeoutMs: 30_000,
    }),
    {
      retry: true,
      failureClass: 'network',
      strategy: 'playwright',
      timeoutMs: 5_000,
    },
  );
});

test('next retry decision advances deterministically without repeating adapters', () => {
  assert.equal(
    decideNextLocalRetry({
      requestedStrategy: 'auto',
      completedStrategies: ['native-fetch', 'playwright'],
      browserlessConfigured: true,
      autoUseBrowserless: true,
      lastStatusCode: 500,
      lastError: 'playwright page crashed',
      startedAtMs: 0,
      nowMs: 5_000,
      maxTotalMs: 120_000,
      requestedTimeoutMs: 30_000,
    }).strategy,
    'puppeteer',
  );

  assert.equal(
    decideNextLocalRetry({
      requestedStrategy: 'auto',
      completedStrategies: ['native-fetch', 'playwright', 'puppeteer'],
      browserlessConfigured: true,
      autoUseBrowserless: true,
      lastStatusCode: 500,
      lastError: 'browser navigation timeout',
      startedAtMs: 0,
      nowMs: 5_000,
      maxTotalMs: 120_000,
      requestedTimeoutMs: 30_000,
    }).strategy,
    'browserless',
  );
});

test('retry decision stops for explicit strategies, terminal failures, exhausted plans, and exhausted budgets', () => {
  const base = {
    browserlessConfigured: false,
    autoUseBrowserless: false,
    startedAtMs: 0,
    maxTotalMs: 120_000,
    requestedTimeoutMs: 30_000,
  } as const;

  assert.deepEqual(
    decideNextLocalRetry({
      ...base,
      requestedStrategy: 'playwright',
      completedStrategies: ['playwright'],
      lastStatusCode: 502,
      lastError: 'network timeout',
      nowMs: 1_000,
    }),
    { retry: false, failureClass: 'timeout', reason: 'explicit-strategy' },
  );

  assert.deepEqual(
    decideNextLocalRetry({
      ...base,
      requestedStrategy: 'auto',
      completedStrategies: ['native-fetch'],
      lastStatusCode: 500,
      lastError: 'SSRF private address blocked',
      nowMs: 1_000,
    }),
    { retry: false, failureClass: 'policy', reason: 'terminal-failure' },
  );

  assert.deepEqual(
    decideNextLocalRetry({
      ...base,
      requestedStrategy: 'auto',
      completedStrategies: ['native-fetch', 'playwright', 'puppeteer'],
      lastStatusCode: 500,
      lastError: 'browser navigation timeout',
      nowMs: 1_000,
    }),
    { retry: false, failureClass: 'timeout', reason: 'plan-exhausted' },
  );

  assert.deepEqual(
    decideNextLocalRetry({
      ...base,
      requestedStrategy: 'auto',
      completedStrategies: ['native-fetch'],
      lastStatusCode: 502,
      lastError: 'fetch failed: ECONNRESET',
      nowMs: 119_750,
    }),
    { retry: false, failureClass: 'network', reason: 'budget-exhausted' },
  );
});
