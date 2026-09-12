import assert from 'node:assert/strict';
import test from 'node:test';

import { decideProviderBudget } from '../src/supervisor-provider-budget.js';

test('provider timeout is capped by elapsed end-to-end wall-clock budget', () => {
  assert.deepEqual(
    decideProviderBudget({
      startedAtMs: 1_000,
      nowMs: 101_000,
      maxTotalMs: 120_000,
      configuredProviderTimeoutMs: 90_000,
      providerDelayMs: 1_000,
    }),
    { eligible: true, timeoutMs: 19_000, remainingBeforeDelayMs: 20_000 },
  );
});

test('provider delay is charged before the network call', () => {
  assert.deepEqual(
    decideProviderBudget({
      startedAtMs: 0,
      nowMs: 118_500,
      maxTotalMs: 120_000,
      configuredProviderTimeoutMs: 90_000,
      providerDelayMs: 1_000,
    }),
    { eligible: false, timeoutMs: 0, remainingBeforeDelayMs: 1_500, reason: 'budget-exhausted' },
  );
});

test('configured provider timeout remains an upper bound when plenty of budget remains', () => {
  assert.deepEqual(
    decideProviderBudget({
      startedAtMs: 0,
      nowMs: 5_000,
      maxTotalMs: 120_000,
      configuredProviderTimeoutMs: 60_000,
      providerDelayMs: 1_000,
    }),
    { eligible: true, timeoutMs: 60_000, remainingBeforeDelayMs: 115_000 },
  );
});

test('minimum provider timeout fails closed instead of starting a doomed call', () => {
  assert.deepEqual(
    decideProviderBudget({
      startedAtMs: 0,
      nowMs: 118_100,
      maxTotalMs: 120_000,
      configuredProviderTimeoutMs: 60_000,
      providerDelayMs: 1_000,
      minimumProviderTimeoutMs: 1_000,
    }),
    { eligible: false, timeoutMs: 0, remainingBeforeDelayMs: 1_900, reason: 'budget-exhausted' },
  );
});

test('invalid provider budget values fail closed', () => {
  assert.throws(
    () => decideProviderBudget({
      startedAtMs: 0,
      nowMs: 1_000,
      maxTotalMs: 120_000,
      configuredProviderTimeoutMs: -1,
      providerDelayMs: 0,
    }),
    TypeError,
  );
  assert.throws(
    () => decideProviderBudget({
      startedAtMs: 0,
      nowMs: 1_000,
      maxTotalMs: 120_000,
      configuredProviderTimeoutMs: 60_000,
      providerDelayMs: Number.NaN,
    }),
    TypeError,
  );
});
