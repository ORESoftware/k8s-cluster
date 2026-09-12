export const LOCAL_BROWSER_RETRY_STRATEGIES = ['playwright', 'puppeteer'] as const;

export type LocalBrowserRetryStrategy = (typeof LOCAL_BROWSER_RETRY_STRATEGIES)[number];
export type LocalRetryFailureClass =
  | 'timeout'
  | 'network'
  | 'browser'
  | 'extraction'
  | 'provider'
  | 'policy-or-access'
  | 'unknown';

export type LocalRetryDecisionReason =
  | 'eligible'
  | 'explicit-strategy'
  | 'policy-or-access-error'
  | 'non-retriable-error'
  | 'already-exhausted';

export type LocalRetryDecision = {
  eligible: boolean;
  reason: LocalRetryDecisionReason;
  failureClass: LocalRetryFailureClass;
  strategies: LocalBrowserRetryStrategy[];
};

const POLICY_OR_ACCESS_ERROR =
  /(robots(?:\.txt)?|captcha|turnstile|recaptcha|hcaptcha|challenge|paywall|sign[ -]?in|log[ -]?in|authentication|authorization|unauthorized|forbidden|private network|metadata|policy|sensitive header|url credentials|access denied|not allowed|blocked by|operator authorization|proxy host)/i;

export function classifyLocalRetryFailure(message: string): LocalRetryFailureClass {
  if (POLICY_OR_ACCESS_ERROR.test(message)) return 'policy-or-access';
  if (/(apify|browserless|actor api|provider)/i.test(message)) return 'provider';
  if (/(timed out|timeout|etimedout|aborterror)/i.test(message)) return 'timeout';
  if (/(extraction worker|parser worker|selector extraction|worker (?:exited|failed))/i.test(message)) {
    return 'extraction';
  }
  if (/(browser|chromium|playwright|puppeteer|navigation|page crashed|target closed|protocol error|failed to launch)/i.test(message)) {
    return 'browser';
  }
  if (/(fetch failed|failed to fetch|network|socket|econn|dns|enotfound|eai_again|ehostunreach|enetunreach|tls|certificate)/i.test(message)) {
    return 'network';
  }
  return 'unknown';
}

export function buildLocalBrowserRetryPlan(input: {
  requestedStrategy: string | undefined;
  initialStrategy: string | undefined;
  localErrorMessage: string;
  attemptedStrategies?: readonly string[];
}): LocalRetryDecision {
  const failureClass = classifyLocalRetryFailure(input.localErrorMessage);
  const requestedStrategy = (input.requestedStrategy ?? 'auto').toLowerCase();
  if (requestedStrategy !== 'auto') {
    return { eligible: false, reason: 'explicit-strategy', failureClass, strategies: [] };
  }
  if (failureClass === 'policy-or-access') {
    return { eligible: false, reason: 'policy-or-access-error', failureClass, strategies: [] };
  }
  if (failureClass === 'unknown') {
    return { eligible: false, reason: 'non-retriable-error', failureClass, strategies: [] };
  }

  const attempted = new Set<string>(input.attemptedStrategies ?? []);
  if (input.initialStrategy) attempted.add(input.initialStrategy);
  const strategies = LOCAL_BROWSER_RETRY_STRATEGIES.filter((strategy) => !attempted.has(strategy));
  if (strategies.length === 0) {
    return { eligible: false, reason: 'already-exhausted', failureClass, strategies: [] };
  }
  return { eligible: true, reason: 'eligible', failureClass, strategies };
}

export function remainingRetryTimeoutMs(input: {
  maxTotalTimeoutMs: number;
  elapsedMs: number;
  requestedTimeoutMs: number;
  reserveMs?: number;
  minimumAttemptMs?: number;
}): number {
  const reserveMs = Math.max(0, Math.floor(input.reserveMs ?? 0));
  const minimumAttemptMs = Math.max(1, Math.floor(input.minimumAttemptMs ?? 1_000));
  const remainingMs = Math.floor(input.maxTotalTimeoutMs - input.elapsedMs - reserveMs);
  if (remainingMs < minimumAttemptMs) return 0;
  return Math.min(Math.floor(input.requestedTimeoutMs), remainingMs);
}
