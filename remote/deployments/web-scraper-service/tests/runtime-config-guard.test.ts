import assert from 'node:assert/strict';
import test from 'node:test';

import { assertTrustedProviderEndpoints } from '../src/runtime-config-guard.js';

test('accepts the canonical Apify v2 endpoint', () => {
  assert.doesNotThrow(() =>
    assertTrustedProviderEndpoints({ APIFY_API_BASE_URL: 'https://api.apify.com/v2' }),
  );
  assert.doesNotThrow(() => assertTrustedProviderEndpoints({}));
});

test('rejects alternate hosts before APIFY_TOKEN can be sent', () => {
  assert.throws(
    () => assertTrustedProviderEndpoints({ APIFY_API_BASE_URL: 'https://example.com/v2' }),
    /exactly https:\/\/api\.apify\.com\/v2/,
  );
});

test('rejects credentials, query strings, fragments, and non-v2 paths', () => {
  for (const value of [
    'https://user:pass@api.apify.com/v2',
    'https://api.apify.com/v2?redirect=example.com',
    'https://api.apify.com/v2#fragment',
    'https://api.apify.com/v1',
    'http://api.apify.com/v2',
  ]) {
    assert.throws(() => assertTrustedProviderEndpoints({ APIFY_API_BASE_URL: value }));
  }
});
