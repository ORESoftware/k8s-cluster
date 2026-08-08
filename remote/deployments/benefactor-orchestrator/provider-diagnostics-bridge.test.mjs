import assert from 'node:assert/strict';
import test from 'node:test';

import {
  buildProviderDiagnostics,
  createProviderDiagnosticsFetch,
  installProviderDiagnostics,
  providerFailureCode,
  providerForRequest,
} from './provider-diagnostics-bridge.mjs';

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
  assert.equal(providerFailureCode({ message: 'secret response body should not be retained' }), 'unknown');
});

test('fetch wrapper counts successes and categorized failures per provider', async () => {
  const state = {
    brave: { requests: 0, successes: 0, failures: 0, failureCodes: {} },
    serper: { requests: 0, successes: 0, failures: 0, failureCodes: {} },
  };
  let call = 0;
  const wrapped = createProviderDiagnosticsFetch(async (input) => {
    call += 1;
    if (String(input).includes('brave')) return new Response('{}', { status: 200 });
    if (call === 2) return new Response('{"message":"quota key must not leak"}', { status: 429 });
    const error = new Error('fetch failed for token=super-secret');
    error.code = 'ECONNRESET';
    throw error;
  }, state);

  await wrapped('https://api.search.brave.com/res/v1/web/search?q=one');
  await wrapped('https://google.serper.dev/search');
  await assert.rejects(wrapped('https://google.serper.dev/search'));

  assert.deepEqual(buildProviderDiagnostics(state), {
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

test('non-provider traffic is passed through without diagnostic mutation', async () => {
  const state = {
    brave: { requests: 0, successes: 0, failures: 0, failureCodes: {} },
    serper: { requests: 0, successes: 0, failures: 0, failureCodes: {} },
  };
  const wrapped = createProviderDiagnosticsFetch(async () => new Response('ok'), state);
  const response = await wrapped('http://dd-web-scraper.default.svc.cluster.local:8097/scrape');
  assert.equal(await response.text(), 'ok');
  assert.deepEqual(buildProviderDiagnostics(state).providers.map((item) => item.requests), [0, 0]);
});

test('installed bridge emits diagnostics only after a pipeline report', async () => {
  const lines = [];
  const target = {
    fetch: async () => new Response('{}', { status: 403 }),
    console: { log: (...args) => lines.push(args.join(' ')) },
  };
  assert.equal(installProviderDiagnostics({ target }), true);
  assert.equal(installProviderDiagnostics({ target }), false);

  await target.fetch('https://google.serper.dev/search');
  target.console.log('ordinary log');
  target.console.log('BENEFACTOR_PIPELINE_REPORT {"reportDigest":"sha256:synthetic"}');

  assert.equal(lines.filter((line) => line.startsWith('BENEFACTOR_PROVIDER_DIAGNOSTICS ')).length, 1);
  const diagnostic = JSON.parse(
    lines.find((line) => line.startsWith('BENEFACTOR_PROVIDER_DIAGNOSTICS ')).split(' ', 2)[1],
  );
  assert.deepEqual(diagnostic.providers.find((item) => item.provider === 'serper').failureCodes, {
    http_403: 1,
  });
  const serialized = JSON.stringify(diagnostic);
  assert.doesNotMatch(serialized, /synthetic|secret|token|query|url/i);
});
