import assert from 'node:assert/strict';
import test from 'node:test';

import {
  PROVIDER_START_WINDOW_MS,
  pruneProviderStarts,
  tryConsumeProviderStartBudget,
} from '../src/provider-start-budget.js';

test('rolling provider-start budget admits N starts and rejects N+1', () => {
  const starts: number[] = [];
  assert.equal(tryConsumeProviderStartBudget(starts, 1_000, 2), true);
  assert.equal(tryConsumeProviderStartBudget(starts, 2_000, 2), true);
  assert.equal(tryConsumeProviderStartBudget(starts, 3_000, 2), false);
  assert.deepEqual(starts, [1_000, 2_000]);
});

test('start exactly one window old is pruned before admission', () => {
  const now = 100_000;
  const starts = [now - PROVIDER_START_WINDOW_MS, now - PROVIDER_START_WINDOW_MS + 1];
  assert.equal(tryConsumeProviderStartBudget(starts, now, 2), true);
  assert.deepEqual(starts, [now - PROVIDER_START_WINDOW_MS + 1, now]);
});

test('rejected admission does not consume budget', () => {
  const starts = [10_000];
  assert.equal(tryConsumeProviderStartBudget(starts, 20_000, 1), false);
  assert.deepEqual(starts, [10_000]);
});

test('pruning removes every stale timestamp and keeps newer starts', () => {
  const starts = [1, 10_000, 60_000, 60_001, 90_000];
  assert.equal(pruneProviderStarts(starts, 120_000), 2);
  assert.deepEqual(starts, [60_001, 90_000]);
});

test('invalid budget arguments fail closed', () => {
  assert.throws(() => tryConsumeProviderStartBudget([], 1_000, 0), /positive integer/);
  assert.throws(() => tryConsumeProviderStartBudget([], Number.NaN, 1), /nowMs must be finite/);
  assert.throws(() => pruneProviderStarts([], 1_000, 0), /positive integer/);
});
