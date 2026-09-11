import assert from 'node:assert/strict';

const baseUrl = process.env.SCRAPER_SMOKE_BASE_URL ?? 'http://127.0.0.1:18097';
const targetUrl = process.env.SCRAPER_SMOKE_TARGET_URL;

if (!targetUrl) {
  throw new Error('SCRAPER_SMOKE_TARGET_URL is required');
}

const target = new URL(targetUrl);
const fixtureUrl = (path) => new URL(path, target.origin).toString();

async function request(path, init = {}, timeoutMs = 25_000) {
  return fetch(`${baseUrl}${path}`, {
    ...init,
    signal: AbortSignal.timeout(timeoutMs),
  });
}

async function jsonResponse(path, init = {}, timeoutMs = 25_000) {
  const response = await request(path, init, timeoutMs);
  const text = await response.text();
  let body;
  try {
    body = JSON.parse(text);
  } catch {
    throw new Error(`${path} returned non-JSON HTTP ${response.status}: ${text.slice(0, 1_000)}`);
  }
  return { response, body };
}

async function getJson(path) {
  const { response, body } = await jsonResponse(path, {}, 10_000);
  assert.equal(response.ok, true, `${path} returned HTTP ${response.status}: ${JSON.stringify(body)}`);
  return body;
}

async function scrape(strategy, extra = {}, expectedStatus = 200) {
  const { response, body } = await jsonResponse('/scrape', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      url: targetUrl,
      strategy,
      timeoutMs: 15_000,
      includeText: true,
      ...extra,
    }),
  });
  assert.equal(
    response.status,
    expectedStatus,
    `${strategy} returned HTTP ${response.status}, expected ${expectedStatus}: ${JSON.stringify(body)}`,
  );
  return { response, body };
}

async function successfulScrape(strategy, extra = {}) {
  const { body } = await scrape(strategy, extra, 200);
  assert.equal(body.ok, true, `${strategy} did not return ok=true`);
  assert.equal(body.strategy, strategy, `${strategy} response reported strategy=${body.strategy}`);
  return body;
}

const health = await getJson('/healthz');
assert.equal(health.ok, true);

const strategies = await getJson('/strategies');
for (const name of ['native-fetch', 'cheerio', 'jsdom', 'linkedom', 'playwright', 'puppeteer']) {
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

for (const alias of ['/scrape/fallback/status', '/status', '/scrape/status']) {
  const value = await getJson(alias);
  assert.equal(value.ok, true, `${alias} must remain available through the supervisor`);
  if (alias.endsWith('/status') && !alias.includes('fallback')) {
    assert.equal(value.externalFallback?.provider, 'apify', `${alias} omitted externalFallback status`);
  }
}

const nativeResult = await successfulScrape('native-fetch', { includeContacts: true });
assert.match(nativeResult.extraction?.text ?? '', /static-ready/);
assert.equal(nativeResult.extraction?.contacts?.emails?.[0]?.address, 'smoke@example.test');
assert.equal(nativeResult.extraction?.contacts?.phones?.[0]?.e164, '+12125550147');

for (const strategy of ['cheerio', 'jsdom', 'linkedom']) {
  const result = await successfulScrape(strategy, { selector: '#static' });
  assert.equal(result.extraction?.selection?.count, 1, `${strategy} failed static selector extraction`);
  assert.equal(result.extraction?.selection?.text, 'static-ready', `${strategy} changed static DOM semantics`);
}

for (const strategy of ['playwright', 'puppeteer']) {
  const result = await successfulScrape(strategy, { selector: '#dynamic' });
  assert.equal(result.extraction?.selection?.count, 1, `${strategy} did not observe the rendered DOM node`);
  assert.equal(result.extraction?.selection?.text, 'dynamic-ready', `${strategy} did not execute page JavaScript`);
}

const { response: autoResponse, body: autoResult } = await jsonResponse('/scrape', {
  method: 'POST',
  headers: { 'content-type': 'application/json' },
  body: JSON.stringify({
    url: targetUrl,
    strategy: 'auto',
    renderJavaScript: true,
    selector: '#dynamic',
    timeoutMs: 15_000,
  }),
});
assert.equal(autoResponse.ok, true, `auto browser scrape failed: ${JSON.stringify(autoResult)}`);
assert.equal(autoResult.ok, true);
assert.equal(autoResult.strategy, 'playwright', 'auto + renderJavaScript should choose local Playwright by default');
assert.equal(autoResult.extraction?.selection?.text, 'dynamic-ready');

for (const strategy of ['native-fetch', 'playwright', 'puppeteer']) {
  const redirected = await successfulScrape(strategy, {
    url: fixtureUrl('/redirect'),
    selector: strategy === 'native-fetch' ? undefined : '#static',
  });
  assert.match(
    redirected.extraction?.text ?? redirected.extraction?.selection?.text ?? '',
    /static-ready/,
    `${strategy} did not follow the same-origin redirect`,
  );
}

const robotsDenied = await scrape('native-fetch', { url: fixtureUrl('/blocked.html') }, 400);
assert.match(robotsDenied.body.error ?? '', /robots\.txt disallows/i);
assert.equal(robotsDenied.body.fallback, undefined, 'robots denial must not be converted into external fallback');

const robotsOverrideDenied = await scrape('native-fetch', { respectRobots: false }, 400);
assert.match(robotsOverrideDenied.body.error ?? '', /robots\.txt override is blocked/i);

const unsupportedProtocol = await scrape('native-fetch', { url: 'file:///etc/passwd' }, 400);
assert.match(unsupportedProtocol.body.error ?? '', /only http and https URLs are supported/i);

const credentialTarget = new URL(targetUrl);
credentialTarget.username = 'runtime-smoke-user';
credentialTarget.password = 'runtime-smoke-secret';
const credentialDenied = await scrape('native-fetch', { url: credentialTarget.toString() }, 400);
assert.match(credentialDenied.body.error ?? '', /URL credentials are blocked by scraper policy/i);
assert.equal(credentialDenied.body.fallback, undefined, 'URL credential denial must never trigger fallback');
assert.doesNotMatch(JSON.stringify(credentialDenied.body), /runtime-smoke-secret/);

const sensitiveHeaderDenied = await scrape(
  'native-fetch',
  { headers: { authorization: 'Bearer runtime-smoke-sensitive-value' } },
  400,
);
assert.match(sensitiveHeaderDenied.body.error ?? '', /blocked sensitive outbound header: authorization/i);
assert.equal(sensitiveHeaderDenied.body.fallback, undefined, 'sensitive-header denial must never trigger fallback');
assert.doesNotMatch(JSON.stringify(sensitiveHeaderDenied.body), /runtime-smoke-sensitive-value/);

const badStrategy = await jsonResponse('/scrape', {
  method: 'POST',
  headers: { 'content-type': 'application/json' },
  body: JSON.stringify({ url: targetUrl, strategy: 'definitely-not-a-strategy' }),
});
assert.equal(badStrategy.response.status, 400, `unknown strategy returned ${badStrategy.response.status}`);
assert.equal(badStrategy.body.ok, false);

const concurrencyRequests = Array.from({ length: 6 }, () =>
  jsonResponse('/scrape', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      url: fixtureUrl('/slow.html?ms=1500'),
      strategy: 'native-fetch',
      timeoutMs: 8_000,
      includeText: true,
    }),
  }, 15_000),
);
const concurrencyResults = await Promise.all(concurrencyRequests);
const concurrencyStatuses = concurrencyResults.map(({ response }) => response.status);
assert.ok(concurrencyStatuses.includes(200), `concurrency probe had no successful requests: ${concurrencyStatuses}`);
assert.ok(concurrencyStatuses.includes(429), `max-concurrent=2 did not shed load: ${concurrencyStatuses}`);
for (const { response, body } of concurrencyResults) {
  assert.ok([200, 429].includes(response.status), `unexpected concurrency status ${response.status}: ${JSON.stringify(body)}`);
  if (response.status === 429) {
    assert.match(body.error ?? '', /scraper concurrency limit reached/i);
    assert.equal(body.maxConcurrent, 2);
  }
}

const oversizedBody = await jsonResponse('/scrape', {
  method: 'POST',
  headers: { 'content-type': 'application/json' },
  body: JSON.stringify({
    url: targetUrl,
    strategy: 'native-fetch',
    padding: 'x'.repeat(4_096),
  }),
});
assert.equal(oversizedBody.response.status, 413, `oversized supervisor request returned ${oversizedBody.response.status}`);
assert.match(oversizedBody.body.error ?? '', /request body exceeds/i);

const largeResponse = await jsonResponse('/scrape', {
  method: 'POST',
  headers: { 'content-type': 'application/json' },
  body: JSON.stringify({
    url: fixtureUrl('/large.html'),
    strategy: 'native-fetch',
    timeoutMs: 15_000,
    includeHtml: true,
  }),
});
assert.equal(largeResponse.response.status, 502, `oversized core response returned ${largeResponse.response.status}`);
assert.equal(largeResponse.body.ok, false);
assert.match(largeResponse.body.error ?? '', /scraper supervisor request failed/i);

for (const path of ['/metrics', '/scrape/metrics']) {
  const response = await request(path, {}, 10_000);
  const text = await response.text();
  assert.equal(response.ok, true, `${path} returned HTTP ${response.status}`);
  assert.match(text, /dd_web_scraper_in_flight/);
  assert.match(text, /dd_web_scraper_robots_denials_total/);
  assert.match(text, /dd_web_scraper_external_fallback_attempts_total\{provider="apify"\}/);
  assert.match(text, /dd_web_scraper_external_fallback_in_flight\{provider="apify"\}/);
}

console.log(JSON.stringify({
  ok: true,
  baseUrl,
  targetUrl,
  strategies: [
    'native-fetch',
    'cheerio',
    'jsdom',
    'linkedom',
    'playwright',
    'puppeteer',
    'auto->playwright',
  ],
  adversarialChecks: [
    'redirects',
    'robots-denial',
    'robots-override-denial',
    'protocol-policy',
    'url-credential-denial-and-redaction',
    'sensitive-header-denial-and-redaction',
    'strategy-validation',
    'concurrency-load-shedding',
    'supervisor-request-cap',
    'supervisor-response-cap',
    'metrics-and-status-aliases',
  ],
}));
