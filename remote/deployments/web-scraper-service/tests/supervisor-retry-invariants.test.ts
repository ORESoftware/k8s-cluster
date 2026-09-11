import assert from 'node:assert/strict';
import test from 'node:test';

import {
  decideNextLocalRetry,
  remainingBudgetMs,
  type LocalScrapeStrategy,
  type RequestedScrapeStrategy,
} from '../src/supervisor-retry-policy.js';

const statuses = [400, 401, 403, 408, 429, 499, 500, 502, 503, 599, 600];
const failures = [
  'network timeout',
  'fetch failed: ECONNRESET',
  'playwright page crashed',
  'parser worker exited unexpectedly',
  'SSRF private address blocked',
  'captcha challenge detected',
  'unclassified failure',
];
const requests: RequestedScrapeStrategy[] = ['auto', 'native-fetch', 'playwright', 'puppeteer', 'browserless'];
const completedSets: LocalScrapeStrategy[][] = [
  [],
  ['native-fetch'],
  ['native-fetch', 'playwright'],
  ['native-fetch', 'playwright', 'puppeteer'],
  ['native-fetch', 'playwright', 'puppeteer', 'browserless'],
];

test('retry decisions preserve global safety invariants across the input matrix', () => {
  let cases = 0;
  for (const statusCode of statuses) {
    for (const lastError of failures) {
      for (const requestedStrategy of requests) {
        for (const completedStrategies of completedSets) {
          for (const aborted of [false, true]) {
            for (const retryCount of [0, 1, 3]) {
              const startedAtMs = 10_000;
              const nowMs = retryCount === 3 ? 129_750 : 25_000;
              const maxTotalMs = 120_000;
              const requestedTimeoutMs = 30_000;
              const maxRetries = 3;
              const decision = decideNextLocalRetry({
                requestedStrategy,
                completedStrategies,
                browserlessConfigured: true,
                autoUseBrowserless: true,
                lastStatusCode: statusCode,
                lastError,
                startedAtMs,
                nowMs,
                maxTotalMs,
                requestedTimeoutMs,
                retryCount,
                maxRetries,
                aborted,
              });
              cases += 1;

              if (aborted) {
                assert.deepEqual(decision.retry, false);
                if (!decision.retry) assert.equal(decision.reason, 'aborted');
                continue;
              }
              if (requestedStrategy !== 'auto') {
                assert.equal(decision.retry, false);
                if (!decision.retry) assert.equal(decision.reason, 'explicit-strategy');
                continue;
              }
              if (retryCount >= maxRetries) {
                if (statusCode >= 500 && statusCode <= 599 && !/SSRF|captcha|unclassified/i.test(lastError)) {
                  assert.equal(decision.retry, false);
                }
              }

              if (decision.retry) {
                assert.ok(statusCode >= 500 && statusCode <= 599);
                assert.ok(retryCount < maxRetries);
                assert.equal(completedStrategies.includes(decision.strategy), false);
                assert.ok(decision.timeoutMs > 0);
                assert.ok(decision.timeoutMs <= requestedTimeoutMs);
                assert.ok(decision.timeoutMs <= remainingBudgetMs(startedAtMs, nowMs, maxTotalMs));
              }
            }
          }
        }
      }
    }
  }

  assert.equal(cases, statuses.length * failures.length * requests.length * completedSets.length * 2 * 3);
  assert.ok(cases >= 10_000, `expected a broad invariant matrix, got ${cases}`);
});
