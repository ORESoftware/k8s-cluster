import assert from 'node:assert/strict';
import test from 'node:test';

import {
  boundedAttemptTimeoutMs,
  buildLocalRetryPlan,
  classifyLocalFailure,
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
