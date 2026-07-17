import assert from 'node:assert/strict';
import { test } from 'node:test';

import { detectCaptcha } from '../src/captcha.js';
import { parseProxyEntry, parseProxyList, ProxyPool } from '../src/proxy-pool.js';

test('captcha detection identifies supported widgets without matching ordinary pages', () => {
  assert.deepEqual(detectCaptcha('<main>ordinary public page</main>'), {
    detected: false,
    type: null,
    sitekey: null,
    action: null,
    signals: [],
  });

  const turnstile = detectCaptcha(
    '<div class="cf-turnstile" data-sitekey="owned-test-site-key"></div>',
  );
  assert.equal(turnstile.detected, true);
  assert.equal(turnstile.type, 'turnstile');
  assert.equal(turnstile.sitekey, 'owned-test-site-key');
});

test('cloudflare interstitial detection stays non-solvable without a sitekey', () => {
  const detection = detectCaptcha(
    '<html><head><title>Just a moment...</title></head><body>Checking your browser</body></html>',
  );
  assert.equal(detection.detected, true);
  assert.equal(detection.type, 'cloudflare-challenge');
  assert.equal(detection.sitekey, null);
});

test('proxy labels redact credentials while preserving connection credentials', () => {
  const entry = parseProxyEntry('https://operator:sensitive-value@proxy.example:8443');
  assert.equal(entry.label, 'https://proxy.example:8443');
  assert.equal(entry.username, 'operator');
  assert.equal(entry.password, 'sensitive-value');
  assert.equal(entry.url.includes('sensitive-value'), true);
  assert.equal(entry.label.includes('sensitive-value'), false);
});

test('proxy parsing deduplicates and rotation cools a failed proxy', () => {
  const entries = parseProxyList(
    'proxy-a.example:8080, proxy-b.example:8080 proxy-a.example:8080',
  );
  assert.equal(entries.length, 2);

  const pool = new ProxyPool(entries, 'round-robin', 60_000);
  const first = pool.select();
  assert.equal(first?.label, 'http://proxy-a.example:8080');
  pool.reportFailure(first!);
  assert.equal(pool.select()?.label, 'http://proxy-b.example:8080');
  assert.deepEqual(pool.stats(), {
    size: 2,
    available: 1,
    cooling: 1,
    rotation: 'round-robin',
    selections: 2,
    failures: 1,
  });
});
