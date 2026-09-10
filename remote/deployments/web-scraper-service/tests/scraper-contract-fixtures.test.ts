import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import { buildApifyActorInput, readApifyFallbackConfig } from '../src/apify-fallback.js';

function loadJson(relativePath: string): Record<string, unknown> {
  return JSON.parse(readFileSync(new URL(relativePath, import.meta.url), 'utf8')) as Record<string, unknown>;
}

test('recorded ScrapeFallbackRequest instance crosses the runtime adapter without widening fields', () => {
  const request = loadJson('../contracts/instances/ScrapeFallbackRequest/valid/minimal.json');
  const config = readApifyFallbackConfig({
    APIFY_FALLBACK_ENABLED: 'true',
    APIFY_TOKEN: 'unit-test-token',
    APIFY_ACTOR_ID: 'apify/web-scraper',
    APIFY_API_BASE_URL: 'https://api.apify.com/v2',
  });

  const actorInput = buildApifyActorInput(request, config);
  assert.deepEqual(actorInput.startUrls, [{ url: 'https://example.com/' }]);
  assert.equal(actorInput.respectRobotsTxtFile, true);
  assert.equal(actorInput.maxPagesPerCrawl, 1);
  assert.equal(actorInput.maxResultsPerCrawl, 1);
  assert.equal(actorInput.maxConcurrency, 1);

  const serialized = JSON.stringify(actorInput);
  assert.equal(serialized.includes('unit-test-token'), false);
  assert.equal(serialized.includes('authorization'), false);
});

test('recorded fallback provenance remains provider-scoped and secret-free', () => {
  const provenance = loadJson('../contracts/instances/FallbackProvenance/valid/apify.json');
  assert.equal(provenance.provider, 'apify');
  assert.equal(provenance.trigger, 'local-retriable-5xx');
  assert.equal(Object.hasOwn(provenance, 'token'), false);
  assert.equal(Object.hasOwn(provenance, 'authorization'), false);
});
