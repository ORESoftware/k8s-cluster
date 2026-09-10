import assert from 'node:assert/strict';
import test from 'node:test';

import {
  buildStrategyAttemptPlan,
  classifyScrapeAttemptFailure,
} from '../src/scrape-fallback.js';

test('auto stays local-first and puts Apify last', () => {
  assert.deepEqual(
    buildStrategyAttemptPlan({
      primary: 'native-fetch',
      requestedStrategy: 'auto',
      autoUseBrowserless: true,
      browserlessConfigured: true,
      apifyFallbackEnabled: true,
      apifyConfigured: true,
    }),
    ['native-fetch', 'playwright', 'puppeteer', 'browserless', 'apify'],
  );
});

test('auto does not call Apify unless both fallback and credentials are configured', () => {
  assert.deepEqual(
    buildStrategyAttemptPlan({
      primary: 'playwright',
      requestedStrategy: 'auto',
      autoUseBrowserless: false,
      browserlessConfigured: false,
      apifyFallbackEnabled: true,
      apifyConfigured: false,
    }),
    ['playwright', 'puppeteer'],
  );
});

test('explicit strategies stay exact by default', () => {
  assert.deepEqual(
    buildStrategyAttemptPlan({
      primary: 'cheerio',
      requestedStrategy: 'cheerio',
      autoUseBrowserless: true,
      browserlessConfigured: true,
      apifyFallbackEnabled: true,
      apifyConfigured: true,
    }),
    ['cheerio'],
  );
});

test('explicit strategy may opt into external fallback without adding local retries', () => {
  assert.deepEqual(
    buildStrategyAttemptPlan({
      primary: 'cheerio',
      requestedStrategy: 'cheerio',
      autoUseBrowserless: true,
      browserlessConfigured: true,
      apifyFallbackEnabled: false,
      apifyConfigured: true,
      externalFallback: true,
    }),
    ['cheerio', 'apify'],
  );
});

test('failure classification is bounded and does not expose raw errors', () => {
  assert.equal(classifyScrapeAttemptFailure(new Error('request timed out after 5s')), 'timeout');
  assert.equal(classifyScrapeAttemptFailure(new Error('browser page crashed')), 'browser');
  assert.equal(classifyScrapeAttemptFailure(new Error('Apify Actor API returned 503')), 'provider');
  assert.equal(classifyScrapeAttemptFailure(new Error('DNS ENOTFOUND')), 'network');
  assert.equal(classifyScrapeAttemptFailure(new Error('extraction worker exited')), 'extraction');
  assert.equal(classifyScrapeAttemptFailure(new Error('something else')), 'unknown');
});
