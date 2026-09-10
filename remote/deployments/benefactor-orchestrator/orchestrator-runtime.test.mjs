import assert from 'node:assert/strict';
import test from 'node:test';

import { createBenefactorRuntime } from './orchestrator-runtime.mjs';

function jsonResponse(body, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

test('explicit runtime composes provider diagnostics and scraper contacts without global mutation', async () => {
  const originalGlobalFetch = globalThis.fetch;
  const originalConsoleLog = console.log;
  const originalConsoleWarn = console.warn;
  const scraperUrl = new URL('http://dd-web-scraper.default.svc.cluster.local:8097');
  const scraperRequests = [];

  const runtime = createBenefactorRuntime({
    scraperUrl,
    requireRoleEmail: true,
    fetchImpl: async (input, init = {}) => {
      const url = new URL(String(input));
      if (url.origin === scraperUrl.origin) {
        const payload = JSON.parse(init.body);
        scraperRequests.push(payload);
        return jsonResponse({
          ok: true,
          strategy: payload.strategy,
          extraction: {
            html: '<html><body>Acme Builders</body></html>',
            text: 'Acme Builders',
            contacts: {
              emails: [{ address: 'info@acmebuilders.com' }],
              phones: [{ e164: '+14155551212' }],
            },
          },
        });
      }
      if (url.origin === 'https://google.serper.dev') return jsonResponse({ organic: [] });
      return new Response('ok');
    },
  });

  const scraperResponse = await runtime.fetch(new URL('/scrape', scraperUrl), {
    method: 'POST',
    body: JSON.stringify({
      url: 'https://acmebuilders.com',
      strategy: 'cheerio',
    }),
  });
  const scraperBody = await scraperResponse.json();

  assert.equal(scraperRequests.length, 1);
  assert.equal(scraperRequests[0].includeContacts, true);
  assert.equal(scraperRequests[0].maxEmails, 50);
  assert.match(scraperBody.extraction.html, /mailto:info@acmebuilders\.com/);
  assert.match(scraperBody.extraction.text, /info@acmebuilders\.com/);

  await runtime.fetch('https://google.serper.dev/search');
  assert.equal(
    runtime.recordProviderWarning(
      '[benefactor-pipeline] provider=serper search_failed ResponseLimitError',
    ),
    true,
  );

  const prefix = 'BENEFACTOR_PROVIDER_DIAGNOSTICS ';
  const line = runtime.providerDiagnosticsLine();
  assert.ok(line.startsWith(prefix));
  const diagnostics = JSON.parse(line.slice(prefix.length));
  assert.deepEqual(diagnostics.providers.find((item) => item.provider === 'serper'), {
    provider: 'serper',
    requests: 1,
    successes: 0,
    failures: 1,
    failureCodes: { response_limit: 1 },
  });
  assert.doesNotMatch(JSON.stringify(diagnostics), /acmebuilders|query|url|sensitive/i);

  assert.equal(globalThis.fetch, originalGlobalFetch);
  assert.equal(console.log, originalConsoleLog);
  assert.equal(console.warn, originalConsoleWarn);
});
