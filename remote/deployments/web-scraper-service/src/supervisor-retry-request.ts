import type { LocalScrapeStrategy, RequestedScrapeStrategy } from './supervisor-retry-policy.js';

export type RetryableScrapeBody = Record<string, unknown> & {
  strategy?: unknown;
  timeoutMs?: unknown;
};

export function parseRetryableScrapeBody(body: Buffer): RetryableScrapeBody | null {
  try {
    const parsed = JSON.parse(body.toString('utf8')) as unknown;
    return parsed !== null && typeof parsed === 'object' && !Array.isArray(parsed)
      ? parsed as RetryableScrapeBody
      : null;
  } catch {
    return null;
  }
}

/**
 * The supervisor only widens implicit/explicit `auto` requests. Any other
 * caller-supplied strategy remains exact even when it is an alias or a value
 * the current core does not recognize; the core owns validation of those
 * explicit inputs and the supervisor must not reinterpret them.
 */
export function requestedRetryStrategy(body: RetryableScrapeBody): RequestedScrapeStrategy | null {
  if (body.strategy === undefined) return 'auto';
  if (typeof body.strategy !== 'string') return null;
  const normalized = body.strategy.trim().toLowerCase();
  return normalized === 'auto' ? 'auto' : null;
}

/**
 * Replay the original JSON contract byte-semantically except for the two fields
 * the retry state machine owns. This intentionally does not strip headers,
 * selectors, robots policy, proxy intent, requestId, or contact limits: the
 * existing core re-validates the complete body on every attempt.
 */
export function buildRetryBody(
  original: RetryableScrapeBody,
  strategy: LocalScrapeStrategy,
  timeoutMs: number,
): Buffer {
  if (!Number.isInteger(timeoutMs) || timeoutMs < 500) {
    throw new TypeError('retry timeoutMs must be an integer >= 500');
  }
  return Buffer.from(JSON.stringify({
    ...original,
    strategy,
    timeoutMs,
  }));
}
