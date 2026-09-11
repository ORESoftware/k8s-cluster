import assert from 'node:assert/strict';
import test from 'node:test';

import {
  buildRetryBody,
  parseRetryableScrapeBody,
  requestedRetryStrategy,
} from '../src/supervisor-retry-request.js';

test('missing or explicit auto strategy is eligible for supervisor widening', () => {
  assert.equal(requestedRetryStrategy({ url: 'https://example.com' }), 'auto');
  assert.equal(requestedRetryStrategy({ strategy: 'auto' }), 'auto');
  assert.equal(requestedRetryStrategy({ strategy: ' AUTO ' }), 'auto');
});

test('every non-auto caller strategy remains exact, including aliases and unknown values', () => {
  for (const strategy of ['playwright', 'fetch', 'native', 'browserless.io', 'future-adapter']) {
    assert.equal(requestedRetryStrategy({ strategy }), null, strategy);
  }
  assert.equal(requestedRetryStrategy({ strategy: 42 }), null);
});

test('retry replay preserves the complete caller contract and overrides only strategy and timeout', () => {
  const original = {
    requestId: 'req-123',
    url: 'https://example.com/path',
    strategy: 'auto',
    timeoutMs: 30_000,
    selector: '#main',
    includeText: true,
    respectRobots: true,
    proxy: 'http://proxy.example:8080',
    headers: { 'x-fixture': 'one' },
  };

  const replay = JSON.parse(buildRetryBody(original, 'playwright', 7_500).toString('utf8'));
  assert.deepEqual(replay, {
    ...original,
    strategy: 'playwright',
    timeoutMs: 7_500,
  });
  assert.equal(original.strategy, 'auto');
  assert.equal(original.timeoutMs, 30_000);
});

test('retry replay rejects sub-core-minimum or non-integral timeouts', () => {
  assert.throws(() => buildRetryBody({}, 'playwright', 499), TypeError);
  assert.throws(() => buildRetryBody({}, 'playwright', 500.5), TypeError);
});

test('retry body parser fails closed on malformed, scalar, array, or null JSON', () => {
  for (const value of ['{', 'null', '42', '"auto"', '[]']) {
    assert.equal(parseRetryableScrapeBody(Buffer.from(value)), null, value);
  }
  assert.deepEqual(
    parseRetryableScrapeBody(Buffer.from('{"url":"https://example.com","strategy":"auto"}')),
    { url: 'https://example.com', strategy: 'auto' },
  );
});
