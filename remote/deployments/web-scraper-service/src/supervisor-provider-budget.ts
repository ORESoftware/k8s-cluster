import { boundedAttemptTimeoutMs, remainingBudgetMs } from './supervisor-retry-policy.js';

export type ProviderBudgetInput = {
  startedAtMs: number;
  nowMs: number;
  maxTotalMs: number;
  configuredProviderTimeoutMs: number;
  providerDelayMs: number;
  minimumProviderTimeoutMs?: number;
};

export type ProviderBudgetDecision =
  | { eligible: true; timeoutMs: number; remainingBeforeDelayMs: number }
  | { eligible: false; timeoutMs: 0; remainingBeforeDelayMs: number; reason: 'budget-exhausted' };

/**
 * Allocate an external-provider timeout from the same wall-clock budget used by
 * local attempts. The provider's configured delay is charged before the call,
 * so local retries can never reset or extend the end-to-end request deadline.
 */
export function decideProviderBudget(input: ProviderBudgetInput): ProviderBudgetDecision {
  for (const [name, value] of [
    ['providerDelayMs', input.providerDelayMs],
    ['configuredProviderTimeoutMs', input.configuredProviderTimeoutMs],
  ] as const) {
    if (!Number.isFinite(value) || value < 0) {
      throw new TypeError(`${name} must be a finite non-negative number`);
    }
  }

  const remainingBeforeDelayMs = remainingBudgetMs(input.startedAtMs, input.nowMs, input.maxTotalMs);
  const remainingAfterDelayMs = Math.max(0, remainingBeforeDelayMs - input.providerDelayMs);
  const timeoutMs = boundedAttemptTimeoutMs(
    input.configuredProviderTimeoutMs,
    remainingAfterDelayMs,
    input.minimumProviderTimeoutMs ?? 1_000,
  );

  return timeoutMs === 0
    ? { eligible: false, timeoutMs: 0, remainingBeforeDelayMs, reason: 'budget-exhausted' }
    : { eligible: true, timeoutMs, remainingBeforeDelayMs };
}
