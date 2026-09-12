import assert from 'node:assert/strict';
import test from 'node:test';

import type { ApifyFallbackConfig, ApifyRunResult } from '../src/apify-fallback.js';
import {
  classifyActorFailure,
  isRetryableActorFailure,
  readApifyFallbackChainConfig,
  runApifyFallbackChain,
  selectFallbackActors,
} from '../src/apify-fallback-chain.js';

const baseConfig: ApifyFallbackConfig = {
  enabled: true,
  token: 'test-token',
  actorId: 'apify/web-scraper',
  apiBaseUrl: 'https://api.apify.com/v2',
  timeoutMs: 60_000,
  localDefaultTimeoutMs: 30_000,
  localMaxTimeoutMs: 60_000,
  maxTotalTimeoutMs: 120_000,
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

test('default Actor chain stays bounded to three and deduplicates Actor IDs', () => {
  const config = readApifyFallbackChainConfig(
    {
      APIFY_FALLBACK_ACTORS: 'apify/web-scraper,apify/playwright-scraper,apify/web-scraper',
      APIFY_FALLBACK_MAX_ATTEMPTS: '3',
      APIFY_FALLBACK_CHAIN_MAX_TOTAL_CHARGE_USD: '0.60',
    },
    baseConfig,
  );
  assert.deepEqual(config.defaultActors, ['apify/web-scraper', 'apify/playwright-scraper']);
  assert.equal(config.maxAttempts, 3);
  assert.equal(config.maxChainTotalChargeUsd, 0.6);
});

test('domain routes use exact host before wildcard and longest wildcard suffix', () => {
  const config = readApifyFallbackChainConfig(
    {
      APIFY_FALLBACK_ACTORS: 'apify/web-scraper',
      APIFY_FALLBACK_MAX_ATTEMPTS: '3',
      APIFY_DOMAIN_FALLBACKS_JSON: JSON.stringify({
        '*.example.com': ['apify/playwright-scraper', 'apify/web-scraper'],
        '*.docs.example.com': ['private/web-compatible-a', 'apify/web-scraper'],
        'api.docs.example.com': ['private/web-compatible-b', 'apify/web-scraper'],
      }),
    },
    baseConfig,
  );

  assert.deepEqual(selectFallbackActors('https://api.docs.example.com/path', config), {
    route: 'api.docs.example.com',
    actors: ['private/web-compatible-b', 'apify/web-scraper'],
  });
  assert.deepEqual(selectFallbackActors('https://guide.docs.example.com/path', config), {
    route: '*.docs.example.com',
    actors: ['private/web-compatible-a', 'apify/web-scraper'],
  });
  assert.deepEqual(selectFallbackActors('https://www.example.com/path', config), {
    route: '*.example.com',
    actors: ['apify/playwright-scraper', 'apify/web-scraper'],
  });
  assert.deepEqual(selectFallbackActors('https://example.com/path', config), {
    route: 'default',
    actors: ['apify/web-scraper'],
  });
});

test('unsafe or malformed domain routing configuration fails closed', () => {
  const overlongLabel = `${'a'.repeat(64)}.example.com`;
  for (const value of [
    '{not-json',
    JSON.stringify({ '*.internal': ['apify/web-scraper'] }),
    JSON.stringify({ '127.0.0.1': ['apify/web-scraper'] }),
    JSON.stringify({ '::1': ['apify/web-scraper'] }),
    JSON.stringify({ 'bad-.example.com': ['apify/web-scraper'] }),
    JSON.stringify({ '-bad.example.com': ['apify/web-scraper'] }),
    JSON.stringify({ [overlongLabel]: ['apify/web-scraper'] }),
    JSON.stringify({ '*.example.com': ['not an actor id'] }),
    JSON.stringify({ '*.example.com': 'apify/web-scraper' }),
    JSON.stringify({
      '*.example.com': [
        'apify/web-scraper',
        'apify/playwright-scraper',
        'apify/puppeteer-scraper',
        'private/fourth',
      ],
    }),
  ]) {
    assert.throws(
      () => readApifyFallbackChainConfig({ APIFY_DOMAIN_FALLBACKS_JSON: value }, baseConfig),
      /APIFY_DOMAIN_FALLBACKS_JSON|invalid fallback domain|Actor|array|contain 1\.\.3/,
    );
  }
});

test('aggregate charge policy fails startup if any configured chain cannot receive one cent per attempt', () => {
  assert.throws(
    () =>
      readApifyFallbackChainConfig(
        {
          APIFY_FALLBACK_ACTORS: 'apify/web-scraper,apify/playwright-scraper,apify/puppeteer-scraper',
          APIFY_FALLBACK_MAX_ATTEMPTS: '3',
          APIFY_FALLBACK_CHAIN_MAX_TOTAL_CHARGE_USD: '0.02',
        },
        baseConfig,
      ),
    /must fund at least USD 0\.01 per configured Actor attempt/,
  );

  assert.doesNotThrow(() =>
    readApifyFallbackChainConfig(
      {
        APIFY_FALLBACK_ACTORS: 'apify/web-scraper,apify/playwright-scraper,apify/puppeteer-scraper',
        APIFY_FALLBACK_MAX_ATTEMPTS: '3',
        APIFY_FALLBACK_CHAIN_MAX_TOTAL_CHARGE_USD: '0.03',
      },
      baseConfig,
    ),
  );
});

test('retriable first Actor failure advances to second Actor with aggregate charge and wall-clock bounds', async () => {
  const chain = readApifyFallbackChainConfig(
    {
      APIFY_FALLBACK_ACTORS: 'apify/web-scraper,apify/playwright-scraper',
      APIFY_FALLBACK_MAX_ATTEMPTS: '2',
      APIFY_FALLBACK_CHAIN_MAX_TOTAL_CHARGE_USD: '0.40',
    },
    baseConfig,
  );
  const seen: ApifyFallbackConfig[] = [];
  const runner = async (
    _request: Parameters<typeof runApifyFallbackChain>[0],
    config: ApifyFallbackConfig,
  ): Promise<ApifyRunResult> => {
    seen.push(config);
    if (seen.length === 1) throw new Error('Apify fallback returned HTTP 503: upstream unavailable');
    return {
      durationMs: 12,
      response: {
        ok: true,
        provider: 'apify',
        fallback: {
          provider: 'apify',
          actorId: config.actorId,
          trigger: 'local-retriable-5xx',
          localStatusCode: 500,
          durationMs: 12,
        },
      },
    };
  };

  const result = await runApifyFallbackChain(
    { url: 'https://example.com', strategy: 'auto', timeoutMs: 30_000 },
    baseConfig,
    { statusCode: 500, error: 'navigation timeout' },
    chain,
    runner,
  );

  assert.deepEqual(seen.map((entry) => entry.actorId), ['apify/web-scraper', 'apify/playwright-scraper']);
  assert.ok(seen.every((entry) => entry.maxTotalChargeUsd <= 0.2));
  assert.ok((seen[0]?.timeoutMs ?? Infinity) <= 44_500);
  assert.ok(seen.every((entry) => entry.timeoutMs <= baseConfig.timeoutMs));
  assert.ok(
    seen.every(
      (entry) =>
        entry.maxTotalTimeoutMs === 30_000 + baseConfig.minDelayMs + entry.timeoutMs,
    ),
  );
  assert.equal(result.attempts.length, 2);
  assert.equal(result.attempts[0]?.outcome, 'error');
  assert.equal(result.attempts[0]?.errorClass, 'provider-server-error');
  assert.equal(result.attempts[1]?.outcome, 'success');
  const fallback = result.result.response.fallback as Record<string, unknown>;
  assert.equal(fallback.route, 'default');
  assert.equal(fallback.maxChainAttempts, 2);
  assert.equal(fallback.maxChainChargeUsd, 0.4);
  assert.deepEqual(
    (fallback.attempts as Array<Record<string, unknown>>).map((attempt) => attempt.actorId),
    ['apify/web-scraper', 'apify/playwright-scraper'],
  );
});

test('provider authentication failure stops the chain instead of multiplying bad paid calls', async () => {
  const chain = readApifyFallbackChainConfig(
    {
      APIFY_FALLBACK_ACTORS: 'apify/web-scraper,apify/playwright-scraper,apify/puppeteer-scraper',
      APIFY_FALLBACK_MAX_ATTEMPTS: '3',
    },
    baseConfig,
  );
  let calls = 0;
  const runner = async (): Promise<ApifyRunResult> => {
    calls += 1;
    throw new Error('Apify fallback returned HTTP 401: unauthorized');
  };

  await assert.rejects(
    () =>
      runApifyFallbackChain(
        { url: 'https://example.com', strategy: 'auto' },
        baseConfig,
        { statusCode: 500, error: 'navigation timeout' },
        chain,
        runner,
      ),
    /provider-auth.*HTTP 401|HTTP 401.*provider-auth/,
  );
  assert.equal(calls, 1);
});

test('actor failure classification is conservative around auth and request errors', () => {
  assert.equal(classifyActorFailure('Apify fallback timed out after 5000ms'), 'timeout');
  assert.equal(classifyActorFailure('Apify fallback returned HTTP 429: retry later'), 'rate-limit');
  assert.equal(classifyActorFailure('Apify fallback returned HTTP 503: unavailable'), 'provider-server-error');
  assert.equal(classifyActorFailure('Apify fallback returned no dataset item'), 'empty-result');
  assert.equal(classifyActorFailure('Apify fallback returned HTTP 401: unauthorized'), 'provider-auth');
  assert.equal(classifyActorFailure('Apify fallback returned HTTP 400: invalid input'), 'invalid-request');
  assert.equal(isRetryableActorFailure('provider-auth'), false);
  assert.equal(isRetryableActorFailure('invalid-request'), false);
  assert.equal(isRetryableActorFailure('provider-server-error'), true);
});
