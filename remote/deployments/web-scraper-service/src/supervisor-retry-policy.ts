export const LOCAL_SCRAPE_STRATEGIES = [
  'native-fetch',
  'cheerio',
  'jsdom',
  'linkedom',
  'playwright',
  'puppeteer',
  'browserless',
] as const;

export type LocalScrapeStrategy = (typeof LOCAL_SCRAPE_STRATEGIES)[number];
export type RequestedScrapeStrategy = LocalScrapeStrategy | 'auto';
export type LocalFailureClass =
  | 'policy'
  | 'auth'
  | 'captcha'
  | 'timeout'
  | 'network'
  | 'browser'
  | 'extraction'
  | 'unknown';

export type LocalRetryPlanInput = {
  requestedStrategy: RequestedScrapeStrategy;
  completedStrategies: readonly LocalScrapeStrategy[];
  browserlessConfigured: boolean;
  autoUseBrowserless: boolean;
};

export type NextLocalRetryDecisionInput = LocalRetryPlanInput & {
  lastStatusCode: number;
  lastError: unknown;
  startedAtMs: number;
  nowMs: number;
  maxTotalMs: number;
  requestedTimeoutMs: number;
  minimumTimeoutMs?: number;
};

export type LocalRetryStopReason =
  | 'explicit-strategy'
  | 'terminal-failure'
  | 'plan-exhausted'
  | 'budget-exhausted';

export type LocalRetryDecision =
  | {
      retry: true;
      failureClass: LocalFailureClass;
      strategy: LocalScrapeStrategy;
      timeoutMs: number;
    }
  | {
      retry: false;
      failureClass: LocalFailureClass;
      reason: LocalRetryStopReason;
    };

const POLICY_MARKERS = [
  'robots',
  'ssrf',
  'private network',
  'private address',
  'loopback',
  'link-local',
  'blocked header',
  'sensitive header',
  'url credentials',
  'proxy policy',
] as const;

const AUTH_MARKERS = [
  'unauthorized',
  'forbidden',
  'authentication required',
  'invalid auth',
  'server_auth_secret',
] as const;

const CAPTCHA_MARKERS = [
  'captcha',
  'challenge detected',
  'challenge page',
] as const;

export function classifyLocalFailure(statusCode: number, error: unknown): LocalFailureClass {
  const message = error instanceof Error ? error.message.toLowerCase() : String(error ?? '').toLowerCase();
  const name = error instanceof Error ? error.name.toLowerCase() : '';

  if (AUTH_MARKERS.some((marker) => message.includes(marker)) || statusCode === 401 || statusCode === 403) {
    return 'auth';
  }
  if (CAPTCHA_MARKERS.some((marker) => message.includes(marker))) return 'captcha';
  if (POLICY_MARKERS.some((marker) => message.includes(marker))) return 'policy';
  if (
    name === 'aborterror'
    || message.includes('timed out')
    || message.includes('timeout')
    || message.includes('etimedout')
  ) {
    return 'timeout';
  }
  if (
    message.includes('fetch failed')
    || message.includes('network')
    || message.includes('socket')
    || message.includes('econn')
    || message.includes('dns')
    || message.includes('enotfound')
  ) {
    return 'network';
  }
  if (
    message.includes('browser')
    || message.includes('chromium')
    || message.includes('playwright')
    || message.includes('puppeteer')
    || message.includes('navigation')
    || message.includes('page crashed')
  ) {
    return 'browser';
  }
  if (
    message.includes('extraction worker')
    || message.includes('parser worker')
    || message.includes('selector extraction')
  ) {
    return 'extraction';
  }
  return 'unknown';
}

export function isLocalRetryEligible(statusCode: number, failureClass: LocalFailureClass): boolean {
  if (statusCode < 500 || statusCode > 599) return false;
  return failureClass === 'timeout'
    || failureClass === 'network'
    || failureClass === 'browser'
    || failureClass === 'extraction';
}

export function buildLocalRetryPlan(input: LocalRetryPlanInput): LocalScrapeStrategy[] {
  if (input.requestedStrategy !== 'auto') return [];

  const completed = new Set(input.completedStrategies);
  const plan: LocalScrapeStrategy[] = [];
  for (const strategy of ['playwright', 'puppeteer'] as const) {
    if (!completed.has(strategy)) plan.push(strategy);
  }
  if (input.autoUseBrowserless && input.browserlessConfigured && !completed.has('browserless')) {
    plan.push('browserless');
  }
  return plan;
}

export function remainingBudgetMs(startedAtMs: number, nowMs: number, maxTotalMs: number): number {
  return Math.max(0, maxTotalMs - Math.max(0, nowMs - startedAtMs));
}

/**
 * Cap one local or provider attempt by the remaining wall-clock budget.
 * Returning zero is an explicit stop signal: callers must not start a new
 * attempt when less than the runtime's minimum meaningful timeout remains.
 */
export function boundedAttemptTimeoutMs(
  requestedTimeoutMs: number,
  remainingMs: number,
  minimumTimeoutMs = 500,
): number {
  for (const [name, value] of [
    ['requestedTimeoutMs', requestedTimeoutMs],
    ['remainingMs', remainingMs],
    ['minimumTimeoutMs', minimumTimeoutMs],
  ] as const) {
    if (!Number.isFinite(value) || value < 0) {
      throw new TypeError(`${name} must be a finite non-negative number`);
    }
  }
  if (minimumTimeoutMs === 0) {
    throw new TypeError('minimumTimeoutMs must be greater than zero');
  }

  const bounded = Math.floor(Math.min(requestedTimeoutMs, remainingMs));
  return bounded >= Math.ceil(minimumTimeoutMs) ? bounded : 0;
}

/**
 * Collapse all local retry policy into one state transition. The supervisor
 * should call this after each failed local attempt and obey the result exactly;
 * that keeps explicit strategy requests exact, terminal failures terminal, the
 * adapter order deterministic, and every attempt inside one wall-clock budget.
 */
export function decideNextLocalRetry(input: NextLocalRetryDecisionInput): LocalRetryDecision {
  const failureClass = classifyLocalFailure(input.lastStatusCode, input.lastError);

  if (input.requestedStrategy !== 'auto') {
    return { retry: false, failureClass, reason: 'explicit-strategy' };
  }
  if (!isLocalRetryEligible(input.lastStatusCode, failureClass)) {
    return { retry: false, failureClass, reason: 'terminal-failure' };
  }

  const [strategy] = buildLocalRetryPlan(input);
  if (!strategy) {
    return { retry: false, failureClass, reason: 'plan-exhausted' };
  }

  const remainingMs = remainingBudgetMs(input.startedAtMs, input.nowMs, input.maxTotalMs);
  const timeoutMs = boundedAttemptTimeoutMs(
    input.requestedTimeoutMs,
    remainingMs,
    input.minimumTimeoutMs ?? 500,
  );
  if (timeoutMs === 0) {
    return { retry: false, failureClass, reason: 'budget-exhausted' };
  }

  return { retry: true, failureClass, strategy, timeoutMs };
}
