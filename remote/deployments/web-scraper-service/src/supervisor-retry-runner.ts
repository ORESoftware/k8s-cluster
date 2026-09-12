import {
  decideNextLocalRetry,
  isLocalScrapeStrategy,
  type LocalFailureClass,
  type LocalRetryStopReason,
  type LocalScrapeStrategy,
  type RequestedScrapeStrategy,
} from './supervisor-retry-policy.js';

export type RetryResponseInspection = {
  statusCode: number;
  error: unknown;
  strategy?: unknown;
};

export type RetryAttemptEvidence = {
  retryIndex: number;
  strategy: LocalScrapeStrategy;
  timeoutMs: number;
  triggerFailureClass: LocalFailureClass;
};

export type RetryRunStopReason = LocalRetryStopReason | 'succeeded';

export type RetryRunResult<T> = {
  response: T;
  retries: readonly RetryAttemptEvidence[];
  completedStrategies: readonly LocalScrapeStrategy[];
  stopReason: RetryRunStopReason;
};

export type RetryLocalFailuresInput<T> = {
  initialResponse: T;
  requestedStrategy: RequestedScrapeStrategy;
  browserlessConfigured: boolean;
  autoUseBrowserless: boolean;
  startedAtMs: number;
  maxTotalMs: number;
  requestedTimeoutMs: number;
  maxRetries: number;
  signal?: AbortSignal;
  now?: () => number;
  inspect: (response: T) => RetryResponseInspection;
  execute: (attempt: RetryAttemptEvidence) => Promise<T>;
};

function isSuccessfulStatus(statusCode: number): boolean {
  return Number.isInteger(statusCode) && statusCode >= 200 && statusCode < 400;
}

/**
 * Execute local retry attempts one-at-a-time. This function intentionally has
 * no timers, sleeps, HTTP, or provider logic: the caller owns I/O while this
 * state machine owns sequencing, retry ceilings, total-budget capping, and
 * sticky cancellation. That separation makes the policy deterministic and
 * unit-testable without a browser or network.
 */
export async function retryLocalFailures<T>(input: RetryLocalFailuresInput<T>): Promise<RetryRunResult<T>> {
  const now = input.now ?? Date.now;
  const completed = new Set<LocalScrapeStrategy>();
  const retries: RetryAttemptEvidence[] = [];
  let response = input.initialResponse;

  while (true) {
    const inspected = input.inspect(response);
    if (isLocalScrapeStrategy(inspected.strategy)) completed.add(inspected.strategy);

    if (isSuccessfulStatus(inspected.statusCode)) {
      return {
        response,
        retries,
        completedStrategies: [...completed],
        stopReason: 'succeeded',
      };
    }

    const decision = decideNextLocalRetry({
      requestedStrategy: input.requestedStrategy,
      completedStrategies: [...completed],
      browserlessConfigured: input.browserlessConfigured,
      autoUseBrowserless: input.autoUseBrowserless,
      lastStatusCode: inspected.statusCode,
      lastError: inspected.error,
      startedAtMs: input.startedAtMs,
      nowMs: now(),
      maxTotalMs: input.maxTotalMs,
      requestedTimeoutMs: input.requestedTimeoutMs,
      retryCount: retries.length,
      maxRetries: input.maxRetries,
      aborted: input.signal?.aborted === true,
    });

    if (!decision.retry) {
      return {
        response,
        retries,
        completedStrategies: [...completed],
        stopReason: decision.reason,
      };
    }

    const evidence: RetryAttemptEvidence = {
      retryIndex: retries.length + 1,
      strategy: decision.strategy,
      timeoutMs: decision.timeoutMs,
      triggerFailureClass: decision.failureClass,
    };
    retries.push(evidence);
    completed.add(decision.strategy);
    response = await input.execute(evidence);
  }
}
