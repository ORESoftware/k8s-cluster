import assert from 'node:assert/strict';
import test from 'node:test';

import {
  buildApifyActorInput,
  classifyFallback,
  isSafeFallbackTarget,
  readApifyFallbackConfig,
  runApifyFallback,
  type ApifyFallbackConfig,
} from '../src/apify-fallback.js';

const config: ApifyFallbackConfig = {
  enabled: true,
  token: 'secret-token-for-test',
  actorId: 'apify/web-scraper',
  apiBaseUrl: 'https://api.apify.com/v2',
  timeoutMs: 60_000,
  maxRequestRetries: 2,
  maxTotalChargeUsd: 0.25,
  maxConcurrent: 1,
  minDelayMs: 1_000,
  failureCooldownMs: 60_000,
  forExplicitStrategy: false,
  maxResponseBytes: 2_000_000,
  maxHtmlChars: 1_000_000,
  maxTextChars: 40_000,
  maxLinks: 250,
  maxPhones: 50,
  maxEmails: 50,
  contactRegion: 'US',
};

test('fallback is limited to retriable 5xx failures from auto strategy', () => {
  assert.deepEqual(
    classifyFallback(config, { url: 'https://example.com', strategy: 'auto' }, 500, 'navigation timeout'),
    { eligible: true, reason: 'eligible' },
  );
  assert.equal(
    classifyFallback(config, { url: 'https://example.com', strategy: 'playwright' }, 500, 'navigation timeout').reason,
    'explicit-strategy',
  );
  assert.equal(
    classifyFallback(config, { url: 'https://example.com', strategy: 'auto' }, 400, 'navigation timeout').reason,
    'non-server-error',
  );
});

test('policy, access-control, and CAPTCHA failures never fall through to Apify', () => {
  for (const error of [
    'robots.txt denied this path',
    'CAPTCHA challenge detected',
    'private network target rejected',
    'authorization forbidden',
    'access denied by policy',
  ]) {
    assert.equal(
      classifyFallback(config, { url: 'https://example.com', strategy: 'auto' }, 500, error).reason,
      'policy-or-access-error',
      error,
    );
  }
});

test('fallback target guard rejects raw IP and local-only hosts', () => {
  assert.equal(isSafeFallbackTarget('https://example.com/path'), true);
  assert.equal(isSafeFallbackTarget('http://127.0.0.1/'), false);
  assert.equal(isSafeFallbackTarget('http://169.254.169.254/latest/meta-data'), false);
  assert.equal(isSafeFallbackTarget('http://service.internal/'), false);
  assert.equal(isSafeFallbackTarget('ftp://example.com/file'), false);
  assert.equal(isSafeFallbackTarget('https://user:pass@example.com/'), false);
});

test('Actor input is single-page, bounded, production mode, and always respects robots', () => {
  const input = buildApifyActorInput(
    {
      url: 'https://example.com',
      selector: 'h1',
      includeLinks: true,
      maxHtmlChars: 50_000_000,
      timeoutMs: 999_999,
      respectRobots: false,
    },
    config,
  );
  assert.equal(input.runMode, 'PRODUCTION');
  assert.equal(input.respectRobotsTxtFile, true);
  assert.equal(input.linkSelector, '');
  assert.equal(input.maxPagesPerCrawl, 1);
  assert.equal(input.maxResultsPerCrawl, 1);
  assert.equal(input.maxConcurrency, 1);
  assert.equal(input.maxRequestRetries, 2);
  assert.deepEqual(input.proxyConfiguration, { useApifyProxy: true });
  const customData = input.customData as Record<string, unknown>;
  assert.equal(customData.maxHtmlChars, config.maxHtmlChars);
  assert.equal(input.pageLoadTimeoutSecs, 60);
});

test('REST call keeps token in Authorization header and applies run cost caps', async () => {
  const originalFetch = globalThis.fetch;
  let seenUrl = '';
  let seenAuthorization = '';
  let seenInput: Record<string, unknown> | null = null;
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    seenUrl = String(input);
    seenAuthorization = String(new Headers(init?.headers).get('authorization'));
    seenInput = JSON.parse(String(init?.body)) as Record<string, unknown>;
    return new Response(
      JSON.stringify([
        {
          finalUrl: 'https://example.com/',
          statusCode: 200,
          contentType: 'text/html; charset=utf-8',
          title: 'Example',
          text: 'Example Domain',
          truncated: false,
        },
      ]),
      { status: 200, headers: { 'content-type': 'application/json' } },
    );
  }) as typeof fetch;

  try {
    const result = await runApifyFallback(
      { requestId: 'req-1', url: 'https://example.com', strategy: 'auto' },
      config,
      { statusCode: 500, strategy: 'playwright', error: 'navigation timeout' },
    );
    const url = new URL(seenUrl);
    assert.match(url.pathname, /\/actors\/apify~web-scraper\/run-sync-get-dataset-items$/);
    assert.equal(url.searchParams.get('token'), null);
    assert.equal(url.searchParams.get('maxItems'), '1');
    assert.equal(url.searchParams.get('maxTotalChargeUsd'), '0.25');
    assert.equal(seenAuthorization, 'Bearer secret-token-for-test');
    assert.equal(seenInput?.maxPagesPerCrawl, 1);
    assert.equal(result.response.provider, 'apify');
    assert.equal(result.response.requestId, 'req-1');
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test('env config is disabled without opt-in and does not require token at startup', () => {
  const parsed = readApifyFallbackConfig({});
  assert.equal(parsed.enabled, false);
  assert.equal(parsed.token, null);
  assert.equal(parsed.actorId, 'apify/web-scraper');
});
