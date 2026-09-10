import assert from 'node:assert/strict';
import test from 'node:test';

import {
  buildProviderDiagnostics,
  createProviderDiagnosticsFetch,
  createProviderState,
  formatProviderDiagnosticsLine,
  providerFailureCode,
  providerForRequest,
  recordProviderWarning,
} from './provider-diagnostics-bridge.mjs';

function state() {
  return createProviderState();
}

test('provider request matching is exact and excludes arbitrary URLs', () => {
  assert.equal(providerForRequest('https://api.search.brave.com/res/v1/web/search?q=roofing'), 'brave');
  assert.equal(providerForRequest('https://google.serper.dev/search'), 'serper');
  assert.equal(providerForRequest('https://api.search.brave.com/other'), null);
  assert.equal(providerForRequest('https://example.com/search'), null);
});

test('failure classification is bounded and does not retain error messages', () => {
  assert.equal(providerFailureCode(null, 401), 'http_401');
  assert.equal(providerFailureCode(null, 429), 'http_429');
  assert.equal(providerFailureCode({ name: 'AbortError', message: 'operation aborted' }), 'timeout');
  assert.equal(providerFailureCode({ name: 'ResponseLimitError', message: 'too large' }), 'response_limit');
  assert.equal(providerFailureCode({ code: 'ENOTFOUND', message: 'getaddrinfo ENOTFOUND key.example' }), 'network');
  assert.equal(providerFailureCode({ message: 'sensitive response detail should not be retained' }), 'unknown');
});

test('fetch wrapper counts successful responses and categorized transport failures', async () => {
  const diagnostics = state();
  let call = 0;
  const wrapped = createProviderDiagnosticsFetch(async (input) => {
    call += 1;
    if (String(input).includes('brave')) return new Response('{}', { status: 200 });
    if (call === 2) return new Response('{"message":"quota detail must not leak"}', { status: 429 });
    const error = new Error('fetch failed with sensitive detail');
    error.code = 'ECONNRESET';
    throw error;
  }, diagnostics);

  await wrapped('https://api.search.brave.com/res/v1/web/search?q=one');
  await wrapped('https://google.serper.dev/search');
  await assert.rejects(wrapped('https://google.serper.dev/search'));

  assert.deepEqual(buildProviderDiagnostics(diagnostics), {
    reportVersion: 'benefactor.provider-diagnostics.v1',
    providers: [
      {
        provider: 'brave',
        requests: 1,
        successes: 1,
        failures: 0,
        failureCodes: {},
      },
      {
        provider: 'serper',
        requests: 2,
        successes: 0,
        failures: 2,
        failureCodes: { http_429: 1, network: 1 },
      },
    ],
  });
});

test('bounded warning signal captures response-body failures after HTTP success', async () => {
  const diagnostics = state();
  const wrapped = createProviderDiagnosticsFetch(async () => new Response('{}', { status: 200 }), diagnostics);
  await wrapped('https://google.serper.dev/search');
  assert.equal(
    recordProviderWarning(
      diagnostics,
      '[benefactor-pipeline] provider=serper search_failed ResponseLimitError',
    ),
    true,
  );

  assert.deepEqual(buildProviderDiagnostics(diagnostics).providers.find((item) => item.provider === 'serper'), {
    provider: 'serper',
    requests: 1,
    successes: 0,
    failures: 1,
    failureCodes: { response_limit: 1 },
  });
});

test('transport failure followed by orchestrator warning is not double-counted', async () => {
  const diagnostics = state();
  const wrapped = createProviderDiagnosticsFetch(async () => new Response('{}', { status: 403 }), diagnostics);
  await wrapped('https://google.serper.dev/search');
  recordProviderWarning(diagnostics, '[benefactor-pipeline] provider=serper search_failed Error');

  assert.deepEqual(buildProviderDiagnostics(diagnostics).providers.find((item) => item.provider === 'serper'), {
    provider: 'serper',
    requests: 1,
    successes: 0,
    failures: 1,
    failureCodes: { http_403: 1 },
  });
});

test('non-provider traffic is passed through without diagnostic mutation', async () => {
  const diagnostics = state();
  const wrapped = createProviderDiagnosticsFetch(async () => new Response('ok'), diagnostics);
  const response = await wrapped('http://dd-web-scraper.default.svc.cluster.local:8097/scrape');
  assert.equal(await response.text(), 'ok');
  assert.deepEqual(buildProviderDiagnostics(diagnostics).providers.map((item) => item.requests), [0, 0]);
});

test('explicit diagnostics formatting emits a bounded line without replacing globals', async () => {
  const diagnostics = createProviderState();
  const originalGlobalFetch = globalThis.fetch;
  const originalConsoleLog = console.log;
  const originalConsoleWarn = console.warn;
  const wrapped = createProviderDiagnosticsFetch(
    async () => new Response('{}', { status: 200 }),
    diagnostics,
  );

  await wrapped('https://google.serper.dev/search');
  assert.equal(
    recordProviderWarning(
      diagnostics,
      '[benefactor-pipeline] provider=serper search_failed ResponseLimitError',
    ),
    true,
  );

  const prefix = 'BENEFACTOR_PROVIDER_DIAGNOSTICS ';
  const line = formatProviderDiagnosticsLine(diagnostics);
  assert.ok(line.startsWith(prefix));
  const diagnostic = JSON.parse(line.slice(prefix.length));
  assert.deepEqual(diagnostic.providers.find((item) => item.provider === 'serper'), {
    provider: 'serper',
    requests: 1,
    successes: 0,
    failures: 1,
    failureCodes: { response_limit: 1 },
  });
  assert.doesNotMatch(JSON.stringify(diagnostic), /sensitive|quota|query|url/i);
  assert.equal(globalThis.fetch, originalGlobalFetch);
  assert.equal(console.log, originalConsoleLog);
  assert.equal(console.warn, originalConsoleWarn);
});
