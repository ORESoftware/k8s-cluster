export const FALLBACK_STRATEGIES = [
  'native-fetch',
  'cheerio',
  'jsdom',
  'linkedom',
  'playwright',
  'puppeteer',
  'browserless',
  'apify',
] as const;

export type FallbackStrategyName = (typeof FALLBACK_STRATEGIES)[number];
export type FallbackRequestedStrategy = FallbackStrategyName | 'auto';
export type ScrapeAttemptFailureClass =
  | 'timeout'
  | 'network'
  | 'browser'
  | 'extraction'
  | 'provider'
  | 'unknown';

export type StrategyAttemptPlanInput = {
  primary: FallbackStrategyName;
  requestedStrategy: FallbackRequestedStrategy;
  autoUseBrowserless: boolean;
  browserlessConfigured: boolean;
  apifyFallbackEnabled: boolean;
  apifyConfigured: boolean;
  externalFallback?: boolean;
};

/**
 * Resilience policy inspired by hosted crawler queues, but deliberately local
 * first. Explicit strategy requests remain exact unless the caller opts in to
 * externalFallback; `auto` may retry the other in-house Chromium adapter before
 * optional hosted providers.
 */
export function buildStrategyAttemptPlan(input: StrategyAttemptPlanInput): FallbackStrategyName[] {
  const plan: FallbackStrategyName[] = [input.primary];

  if (input.requestedStrategy === 'auto') {
    if (input.primary !== 'playwright') pushUnique(plan, 'playwright');
    if (input.primary !== 'puppeteer') pushUnique(plan, 'puppeteer');
    if (input.autoUseBrowserless && input.browserlessConfigured) {
      pushUnique(plan, 'browserless');
    }
  }

  const useExternalFallback =
    input.externalFallback ??
    (input.requestedStrategy === 'auto' && input.apifyFallbackEnabled);
  if (useExternalFallback && input.apifyConfigured) {
    pushUnique(plan, 'apify');
  }

  return plan;
}

export function classifyScrapeAttemptFailure(error: unknown): ScrapeAttemptFailureClass {
  const message = error instanceof Error ? error.message.toLowerCase() : String(error).toLowerCase();
  const name = error instanceof Error ? error.name.toLowerCase() : '';

  if (
    name === 'aborterror' ||
    message.includes('timed out') ||
    message.includes('timeout') ||
    message.includes('etimedout')
  ) {
    return 'timeout';
  }
  if (
    message.includes('apify') ||
    message.includes('browserless') ||
    message.includes('actor api')
  ) {
    return 'provider';
  }
  if (
    message.includes('extraction worker') ||
    message.includes('parser worker') ||
    message.includes('selector extraction')
  ) {
    return 'extraction';
  }
  if (
    message.includes('browser') ||
    message.includes('chromium') ||
    message.includes('playwright') ||
    message.includes('puppeteer') ||
    message.includes('navigation') ||
    message.includes('page crashed')
  ) {
    return 'browser';
  }
  if (
    message.includes('fetch failed') ||
    message.includes('network') ||
    message.includes('socket') ||
    message.includes('econn') ||
    message.includes('dns') ||
    message.includes('enotfound')
  ) {
    return 'network';
  }
  return 'unknown';
}

function pushUnique(plan: FallbackStrategyName[], strategy: FallbackStrategyName): void {
  if (!plan.includes(strategy)) plan.push(strategy);
}
