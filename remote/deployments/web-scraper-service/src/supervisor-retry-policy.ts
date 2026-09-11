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
