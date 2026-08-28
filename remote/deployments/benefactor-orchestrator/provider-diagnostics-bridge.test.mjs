import assert from 'node:assert/strict';
import test from 'node:test';

import {
  buildProviderDiagnostics,
  createProviderDiagnosticsFetch,
  installProviderDiagnostics,
  providerFailureCode,
  providerForRequest,
  recordProviderWarning,
} from './provider-diagnostics-bridge.mjs';

function state() {
  return {
    brave: { requests: 0, failures: 0, failureCodes: {}, pendingFailureWarnings: 0 },
    serper: { requests: 0, failures: 0, failureCodes: {}, pendingFailureWarnings: 0 },
  };
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

test('installed bridge emits diagnostics only after a pipeline report', async () => {
  const lines = [];
  const target = {
    fetch: async () => new Response('{}', { status: 200 }),
    console: {
      log: (...args) => lines.push(args.join(' ')),
      warn: (...args) => lines.push(args.join(' ')),
    },
  };
  assert.equal(installProviderDiagnostics({ target }), true);
  assert.equal(installProviderDiagnostics({ target }), false);

  await target.fetch('https://google.serper.dev/search');
  target.console.warn('[benefactor-pipeline] provider=serper search_failed ResponseLimitError');
  target.console.log('ordinary log');
  target.console.log('BENEFACTOR_PIPELINE_REPORT {"reportDigest":"sha256:synthetic"}');

  assert.equal(lines.filter((line) => line.startsWith('BENEFACTOR_PROVIDER_DIAGNOSTICS ')).length, 1);
  const diagnostic = JSON.parse(
    lines.find((line) => line.startsWith('BENEFACTOR_PROVIDER_DIAGNOSTICS ')).split(' ', 2)[1],
  );
  assert.deepEqual(diagnostic.providers.find((item) => item.provider === 'serper').failureCodes, {
    response_limit: 1,
  });
  const serialized = JSON.stringify(diagnostic);
  assert.doesNotMatch(serialized, /synthetic|sensitive|quota|query|url/i);
});
