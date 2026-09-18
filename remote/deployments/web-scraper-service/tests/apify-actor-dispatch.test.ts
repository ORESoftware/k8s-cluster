import assert from 'node:assert/strict';
import test from 'node:test';

import type { ApifyFallbackConfig } from '../src/apify-fallback.js';
import { actorProfile, buildBrowserActorInput } from '../src/apify-actor-dispatch.js';

const config: ApifyFallbackConfig = {
  enabled: true,
  token: 'test-token',
  actorId: 'apify/playwright-scraper',
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

test('known Apify-maintained browser Actors select server-side browser profiles', () => {
  assert.equal(actorProfile('apify/web-scraper'), 'web-scraper');
  assert.equal(actorProfile('apify/playwright-scraper'), 'playwright-scraper');
  assert.equal(actorProfile('apify/puppeteer-scraper'), 'puppeteer-scraper');
  assert.equal(actorProfile('private-org/web-compatible-actor'), 'web-scraper');
});

test('Playwright input is single-page, robots-aware, bounded and carries extraction data in userData', () => {
  const input = buildBrowserActorInput(
    {
      url: 'https://example.com/path',
      selector: 'h1',
      includeHtml: true,
      includeLinks: true,
      includeContacts: true,
      maxHtmlChars: 99_000_000,
      timeoutMs: 999_999,
      waitUntil: 'networkidle',
    },
    config,
    'playwright-scraper',
  );
  assert.equal(input.respectRobotsTxtFile, true);
  assert.equal(input.linkSelector, '');
  assert.equal(input.maxPagesPerCrawl, 1);
  assert.equal(input.maxResultsPerCrawl, 1);
  assert.equal(input.maxConcurrency, 1);
  assert.equal(input.maxRequestRetries, 2);
  assert.equal(input.waitUntil, 'networkidle');
  assert.deepEqual(input.proxyConfiguration, { useApifyProxy: true });

  const startUrl = (input.startUrls as Array<Record<string, any>>)[0];
  assert.equal(startUrl?.url, 'https://example.com/path');
  assert.equal(startUrl?.userData?.dd?.selector, 'h1');
  assert.equal(startUrl?.userData?.dd?.maxHtmlChars, config.maxHtmlChars);
  assert.equal(startUrl?.userData?.dd?.includeLinks, true);
  assert.equal(startUrl?.userData?.dd?.collectContacts, true);
  assert.match(String(input.pageFunction), /page\.evaluate/);
});

test('Puppeteer network-idle spelling is mapped to networkidle2', () => {
  const input = buildBrowserActorInput(
    { url: 'https://example.com', waitUntil: 'networkidle' },
    { ...config, actorId: 'apify/puppeteer-scraper' },
    'puppeteer-scraper',
  );
  assert.equal(input.waitUntil, 'networkidle2');
});

test('browser Actor profiles retain the same conservative target guard', () => {
  assert.throws(
    () =>
      buildBrowserActorInput(
        { url: 'http://127.0.0.1/private' },
        config,
        'playwright-scraper',
      ),
    /target is not eligible/,
  );
});
