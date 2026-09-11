import assert from 'node:assert/strict';
import test from 'node:test';

import { retryLocalFailures } from '../src/supervisor-retry-runner.js';

type FakeResponse = {
  statusCode: number;
  error: string;
  strategy: string;
};

const inspect = (response: FakeResponse) => response;

test('runs retry adapters sequentially and stops immediately on success', async () => {
  const executed: string[] = [];
  const responses: Record<string, FakeResponse> = {
    playwright: { statusCode: 500, error: 'playwright page crashed', strategy: 'playwright' },
    puppeteer: { statusCode: 200, error: '', strategy: 'puppeteer' },
  };
  let now = 1_000;

  const result = await retryLocalFailures({
    initialResponse: { statusCode: 502, error: 'fetch failed: ECONNRESET', strategy: 'native-fetch' },
    requestedStrategy: 'auto',
    browserlessConfigured: true,
    autoUseBrowserless: true,
    startedAtMs: 1_000,
    maxTotalMs: 120_000,
    requestedTimeoutMs: 30_000,
    maxRetries: 3,
    now: () => now,
    inspect,
    execute: async (attempt) => {
      executed.push(attempt.strategy);
      now += 10_000;
      return responses[attempt.strategy] ?? { statusCode: 500, error: 'unexpected', strategy: attempt.strategy };
    },
  });

  assert.deepEqual(executed, ['playwright', 'puppeteer']);
  assert.equal(result.response.statusCode, 200);
  assert.equal(result.stopReason, 'terminal-failure');
  assert.deepEqual(result.completedStrategies, ['native-fetch', 'playwright', 'puppeteer']);
  assert.deepEqual(result.retries.map((attempt) => attempt.retryIndex), [1, 2]);
});

test('remaining wall-clock budget shrinks later attempt timeouts', async () => {
  const timeouts: number[] = [];
  let now = 1_000;

  const result = await retryLocalFailures({
    initialResponse: { statusCode: 502, error: 'network timeout', strategy: 'native-fetch' },
    requestedStrategy: 'auto',
    browserlessConfigured: false,
    autoUseBrowserless: false,
    startedAtMs: 1_000,
    maxTotalMs: 40_000,
    requestedTimeoutMs: 30_000,
    maxRetries: 3,
    now: () => now,
    inspect,
    execute: async (attempt) => {
      timeouts.push(attempt.timeoutMs);
      now += 25_000;
      return { statusCode: 500, error: `${attempt.strategy} navigation timeout`, strategy: attempt.strategy };
    },
  });

  assert.deepEqual(timeouts, [30_000, 15_000]);
  assert.equal(result.stopReason, 'plan-exhausted');
});

test('abort is sticky and prevents the next queued adapter from starting', async () => {
  const controller = new AbortController();
  const executed: string[] = [];

  const result = await retryLocalFailures({
    initialResponse: { statusCode: 502, error: 'network timeout', strategy: 'native-fetch' },
    requestedStrategy: 'auto',
    browserlessConfigured: true,
    autoUseBrowserless: true,
    startedAtMs: 0,
    maxTotalMs: 120_000,
    requestedTimeoutMs: 30_000,
    maxRetries: 3,
    signal: controller.signal,
    now: () => 1_000,
    inspect,
    execute: async (attempt) => {
      executed.push(attempt.strategy);
      controller.abort();
      return { statusCode: 500, error: 'browser timeout', strategy: attempt.strategy };
    },
  });

  assert.deepEqual(executed, ['playwright']);
  assert.equal(result.stopReason, 'aborted');
});

test('hard retry ceiling wins even when additional adapters remain eligible', async () => {
  const executed: string[] = [];

  const result = await retryLocalFailures({
    initialResponse: { statusCode: 502, error: 'network timeout', strategy: 'native-fetch' },
    requestedStrategy: 'auto',
    browserlessConfigured: true,
    autoUseBrowserless: true,
    startedAtMs: 0,
    maxTotalMs: 120_000,
    requestedTimeoutMs: 30_000,
    maxRetries: 1,
    now: () => 1_000,
    inspect,
    execute: async (attempt) => {
      executed.push(attempt.strategy);
      return { statusCode: 500, error: 'browser crashed', strategy: attempt.strategy };
    },
  });

  assert.deepEqual(executed, ['playwright']);
  assert.equal(result.stopReason, 'retry-limit');
});

test('explicit strategy never widens to another adapter', async () => {
  let executions = 0;
  const result = await retryLocalFailures({
    initialResponse: { statusCode: 500, error: 'playwright timeout', strategy: 'playwright' },
    requestedStrategy: 'playwright',
    browserlessConfigured: true,
    autoUseBrowserless: true,
    startedAtMs: 0,
    maxTotalMs: 120_000,
    requestedTimeoutMs: 30_000,
    maxRetries: 3,
    inspect,
    execute: async () => {
      executions += 1;
      throw new Error('must not execute');
    },
  });

  assert.equal(executions, 0);
  assert.equal(result.stopReason, 'explicit-strategy');
});
