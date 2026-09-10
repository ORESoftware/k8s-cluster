import assert from 'node:assert/strict';
import test from 'node:test';

import {
  buildApifyActorInput,
  buildApifyRunUrl,
  fetchWithApifyFallback,
  type ApifyFallbackConfig,
} from '../src/apify-fallback.js';

const config: ApifyFallbackConfig = {
  token: 'test-token',
  actorId: 'apify~website-content-crawler',
  apiBaseUrl: 'https://api.apify.com/v2',
  timeoutMs: 1_000,
  maxResponseBytes: 64 * 1024,
  useApifyProxy: true,
};

const targetUrl = new URL('https://example.com/company');

function jsonResponse(body: unknown, init: ResponseInit = {}): Response {
  return new Response(JSON.stringify(body), {
    status: init.status ?? 200,
    headers: {
      'content-type': 'application/json',
      ...(init.headers ?? {}),
    },
  });
}

test('buildApifyRunUrl keeps the token out of the URL', () => {
  const url = new URL(buildApifyRunUrl(config));
  assert.equal(url.protocol, 'https:');
  assert.equal(
    url.pathname,
    '/v2/actors/apify~website-content-crawler/run-sync-get-dataset-items',
  );
  assert.equal(url.searchParams.get('format'), 'json');
  assert.equal(url.searchParams.get('clean'), 'true');
  assert.equal(url.searchParams.get('limit'), '1');
  assert.equal(url.searchParams.has('token'), false);
});

test('buildApifyActorInput is a one-page robots-respecting crawl', () => {
  const actorInput = buildApifyActorInput(
    { targetUrl, userAgent: 'dd-web-scraper/test' },
    config,
  );
  assert.deepEqual(actorInput.startUrls, [{ url: targetUrl.toString() }]);
  assert.equal(actorInput.maxCrawlDepth, 0);
  assert.equal(actorInput.maxCrawlPages, 1);
  assert.equal(actorInput.respectRobotsTxtFile, true);
  assert.equal(actorInput.useSitemaps, false);
  assert.equal(actorInput.useLlmsTxt, false);
  assert.deepEqual(actorInput.proxyConfiguration, { useApifyProxy: true });
});

test('fetchWithApifyFallback authenticates in a header and returns HTML', async () => {
  let requestUrl = '';
  let requestInit: RequestInit | undefined;
  const fetchImpl = async (input: string | URL | Request, init?: RequestInit) => {
    requestUrl = String(input);
    requestInit = init;
    return jsonResponse([
      {
        url: targetUrl.toString(),
        html: '<html><head><title>Example</title></head><body>hello</body></html>',
        crawl: { loadedUrl: 'https://example.com/company/', httpStatusCode: 200 },
      },
    ]);
  };

  const result = await fetchWithApifyFallback(
    {
      targetUrl,
      maxHtmlChars: 10_000,
      userAgent: 'dd-web-scraper/test',
      requiresHtml: true,
    },
    config,
    fetchImpl,
  );

  const url = new URL(requestUrl);
  assert.equal(url.searchParams.has('token'), false);
  assert.equal(new Headers(requestInit?.headers).get('authorization'), 'Bearer test-token');
  assert.match(String(requestInit?.body), /"maxCrawlPages":1/);
  assert.equal(result.status, 200);
  assert.equal(result.finalUrl, 'https://example.com/company/');
  assert.match(result.html, /<title>Example<\/title>/);
});

test('text-only Apify output is converted to inert HTML for ordinary extraction', async () => {
  const result = await fetchWithApifyFallback(
    {
      targetUrl,
      maxHtmlChars: 10_000,
      userAgent: 'dd-web-scraper/test',
    },
    config,
    async () =>
      jsonResponse([
        {
          url: targetUrl.toString(),
          metadata: { title: '<Example & Co>' },
          text: '<script>not executable</script> Sales: hello@example.com',
        },
      ]),
  );

  assert.match(result.html, /&lt;script&gt;not executable&lt;\/script&gt;/);
  assert.match(result.html, /&lt;Example &amp; Co&gt;/);
});

test('selector fallback fails closed when Apify does not return HTML', async () => {
  await assert.rejects(
    fetchWithApifyFallback(
      {
        targetUrl,
        maxHtmlChars: 10_000,
        userAgent: 'dd-web-scraper/test',
        requiresHtml: true,
      },
      config,
      async () => jsonResponse([{ url: targetUrl.toString(), text: 'plain text only' }]),
    ),
    /did not return HTML required for selector extraction/,
  );
});

test('Apify API base URL must be HTTPS so bearer tokens never cross plaintext HTTP', () => {
  assert.throws(
    () => buildApifyRunUrl({ ...config, apiBaseUrl: 'http://api.apify.com/v2' }),
    /must use https/,
  );
});

test('provider response size is bounded', async () => {
  await assert.rejects(
    fetchWithApifyFallback(
      {
        targetUrl,
        maxHtmlChars: 10_000,
        userAgent: 'dd-web-scraper/test',
      },
      { ...config, maxResponseBytes: 32 },
      async () =>
        new Response(JSON.stringify([{ text: 'x'.repeat(100) }]), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        }),
    ),
    /response exceeded 32 bytes/,
  );
});
