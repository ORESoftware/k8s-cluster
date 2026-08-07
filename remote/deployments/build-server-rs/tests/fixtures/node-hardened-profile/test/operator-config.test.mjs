import test from 'node:test';
import assert from 'node:assert/strict';

import { validateOperatorConfig } from '../src/operator-config.mjs';

test('accepts the reviewed official-API and consent boundary', () => {
  const result = validateOperatorConfig({
    collectionMode: 'official-api',
    consentRequired: true,
    maxPages: 25,
  });
  assert.deepEqual(result, {
    collectionMode: 'official-api',
    consentRequired: true,
    maxPages: 25,
  });
  assert.ok(Object.isFrozen(result));
});

test('rejects scraping, missing consent, unbounded work, and unknown keys', () => {
  for (const value of [
    { collectionMode: 'browser-scrape', consentRequired: true, maxPages: 25 },
    { collectionMode: 'official-api', consentRequired: false, maxPages: 25 },
    { collectionMode: 'official-api', consentRequired: true, maxPages: 0 },
    { collectionMode: 'official-api', consentRequired: true, maxPages: 101 },
    {
      collectionMode: 'official-api',
      consentRequired: true,
      maxPages: 25,
      token: 'must-not-enter-config',
    },
  ]) {
    assert.throws(() => validateOperatorConfig(value), TypeError);
  }
});
