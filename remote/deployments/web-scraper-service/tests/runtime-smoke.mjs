import assert from 'node:assert/strict';

const baseUrl = process.env.SCRAPER_SMOKE_BASE_URL ?? 'http://127.0.0.1:18097';
const targetUrl = process.env.SCRAPER_SMOKE_TARGET_URL;

if (!targetUrl) {
  throw new Error('SCRAPER_SMOKE_TARGET_URL is required');
}

async function getJson(path) {
  const response = await fetch(`${baseUrl}${path}`, { signal: AbortSignal.timeout(10_000) });
  const body = await response.json();
  assert.equal(response.ok, true, `${path} returned HTTP ${response.status}: ${JSON.stringify(body)}`);
  return body;
}

async function scrape(strategy, extra = {}) {
  const response = await fetch(`${baseUrl}/scrape`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      url: targetUrl,
      strategy,
      timeoutMs: 15_000,
      includeText: true,
      ...extra,
    }),
    signal: AbortSignal.timeout(25_000),
  });
  const body = await response.json();
  assert.equal(response.ok, true, `${strategy} returned HTTP ${response.status}: ${JSON.stringify(body)}`);
  assert.equal(body.ok, true, `${strategy} did not return ok=true`);
  assert.equal(body.strategy, strategy, `${strategy} response reported strategy=${body.strategy}`);
  return body;
}

const health = await getJson('/healthz');
assert.equal(health.ok, true);

const strategies = await getJson('/strategies');
for (const name of ['native-fetch', 'cheerio', 'playwright', 'puppeteer']) {
  const descriptor = strategies.strategies?.find((item) => item.name === name);
  assert.ok(descriptor, `missing strategy descriptor for ${name}`);
  assert.equal(descriptor.available, true, `${name} is not available in the final image`);
}

const fallback = await getJson('/fallback/status');
assert.equal(fallback.ok, true, 'fallback status endpoint must report ok=true');
assert.equal(fallback.fallback?.provider, 'apify', 'fallback status must identify the bounded provider');
assert.equal(fallback.fallback?.enabled, true, 'CLI --apify-fallback did not reach the supervisor');
assert.equal(
  fallback.fallback?.configured,
  false,
  'runtime smoke must not require or inject an APIFY_TOKEN',
);

const nativeResult = await scrape('native-fetch', { includeContacts: true });
assert.match(nativeResult.extraction?.text ?? '', /static-ready/);
assert.equal(nativeResult.extraction?.contacts?.emails?.[0]?.address, 'smoke@example.test');
assert.equal(nativeResult.extraction?.contacts?.phones?.[0]?.e164, '+12125550147');

const cheerioResult = await scrape('cheerio', { selector: '#static' });
assert.equal(cheerioResult.extraction?.selection?.text, 'static-ready');

for (const strategy of ['playwright', 'puppeteer']) {
  const result = await scrape(strategy, { selector: '#dynamic' });
  assert.equal(result.extraction?.selection?.count, 1, `${strategy} did not observe the rendered DOM node`);
  assert.equal(result.extraction?.selection?.text, 'dynamic-ready', `${strategy} did not execute page JavaScript`);
}

const autoResponse = await fetch(`${baseUrl}/scrape`, {
  method: 'POST',
  headers: { 'content-type': 'application/json' },
  body: JSON.stringify({
    url: targetUrl,
    strategy: 'auto',
    renderJavaScript: true,
    selector: '#dynamic',
    timeoutMs: 15_000,
  }),
  signal: AbortSignal.timeout(25_000),
});
const autoResult = await autoResponse.json();
assert.equal(autoResponse.ok, true, `auto browser scrape failed: ${JSON.stringify(autoResult)}`);
assert.equal(autoResult.ok, true);
assert.equal(autoResult.strategy, 'playwright', 'auto + renderJavaScript should choose local Playwright by default');
assert.equal(autoResult.extraction?.selection?.text, 'dynamic-ready');

console.log(JSON.stringify({
  ok: true,
  baseUrl,
  targetUrl,
  strategies: ['native-fetch', 'cheerio', 'playwright', 'puppeteer', 'auto->playwright'],
}));
