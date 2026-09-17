import assert from 'node:assert/strict';
import test from 'node:test';

import {
  buildLocalBrowserRetryPlan,
  classifyLocalRetryFailure,
  remainingRetryTimeoutMs,
} from '../src/scrape-retry-policy.js';

test('auto retries local browser engines in deterministic Playwright/Puppeteer order', () => {
  const plan = buildLocalBrowserRetryPlan({
    requestedStrategy: 'auto',
    initialStrategy: 'native-fetch',
    localErrorMessage: 'fetch failed: ECONNRESET',
  });
  assert.deepEqual(plan, {
    eligible: true,
    reason: 'eligible',
    failureClass: 'network',
    strategies: ['playwright', 'puppeteer'],
  });
});

test('auto never repeats an already attempted browser engine', () => {
  const plan = buildLocalBrowserRetryPlan({
    requestedStrategy: 'auto',
    initialStrategy: 'playwright',
    attemptedStrategies: ['native-fetch', 'playwright'],
    localErrorMessage: 'browser page crashed',
  });
  assert.deepEqual(plan.strategies, ['puppeteer']);
});

test('explicit strategies remain exact by default', () => {
  const plan = buildLocalBrowserRetryPlan({
    requestedStrategy: 'cheerio',
    initialStrategy: 'cheerio',
    localErrorMessage: 'request timed out',
  });
  assert.equal(plan.eligible, false);
  assert.equal(plan.reason, 'explicit-strategy');
  assert.deepEqual(plan.strategies, []);
});

test('policy and access failures are terminal even under auto', () => {
  for (const message of [
    'blocked by robots.txt',
    'private network target denied by policy',
    'captcha challenge detected',
    'authorization required',
  ]) {
    const plan = buildLocalBrowserRetryPlan({
      requestedStrategy: 'auto',
      initialStrategy: 'native-fetch',
      localErrorMessage: message,
    });
    assert.equal(plan.eligible, false, message);
    assert.equal(plan.reason, 'policy-or-access-error', message);
  }
});

test('unknown failures do not broaden into browser retries', () => {
  const plan = buildLocalBrowserRetryPlan({
    requestedStrategy: 'auto',
    initialStrategy: 'native-fetch',
    localErrorMessage: 'unexpected application invariant',
  });
  assert.equal(plan.eligible, false);
  assert.equal(plan.reason, 'non-retriable-error');
});

test('failure classification is bounded and never returns raw error material', () => {
  assert.equal(classifyLocalRetryFailure('request timed out after 5s'), 'timeout');
  assert.equal(classifyLocalRetryFailure('browser page crashed'), 'browser');
  assert.equal(classifyLocalRetryFailure('Browserless provider returned 503'), 'provider');
  assert.equal(classifyLocalRetryFailure('DNS ENOTFOUND'), 'network');
  assert.equal(classifyLocalRetryFailure('extraction worker exited'), 'extraction');
  assert.equal(classifyLocalRetryFailure('robots.txt denied'), 'policy-or-access');
  assert.equal(classifyLocalRetryFailure('something else'), 'unknown');
});

test('retry timeout shares the total budget and preserves a provider reserve', () => {
  assert.equal(
    remainingRetryTimeoutMs({
      maxTotalTimeoutMs: 120_000,
      elapsedMs: 30_000,
      requestedTimeoutMs: 60_000,
      reserveMs: 31_000,
    }),
    59_000,
  );
  assert.equal(
    remainingRetryTimeoutMs({
      maxTotalTimeoutMs: 120_000,
      elapsedMs: 118_500,
      requestedTimeoutMs: 60_000,
      reserveMs: 1_000,
    }),
    0,
  );
});
