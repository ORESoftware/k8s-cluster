import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import {
  augmentScraperBody,
  createScraperContactFetch,
  enrichScraperRequest,
  normalizeStructuredContacts,
  shouldEscalateToBrowser,
} from './scraper-contact-bridge.mjs';

const scraperUrl = new URL('http://dd-web-scraper.default.svc.cluster.local:8097');

function response(body, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

test('structured contacts normalize role emails and E.164 phone candidates', () => {
  assert.deepEqual(normalizeStructuredContacts({
    contacts: {
      emails: [
        { address: ' Sales@AcmeBuilders.com ' },
        { address: 'jane@acmebuilders.com' },
        { address: 'owner@gmail.com' },
      ],
      phones: [
        { e164: '+14155551212', raw: '(415) 555-1212' },
        { raw: '+51 987 654 321' },
      ],
    },
  }), {
    emails: ['sales@acmebuilders.com'],
    phones: ['+14155551212', '+51987654321'],
  });
});

test('request enrichment opts into the private scraper contact contract with bounded caps', () => {
  assert.deepEqual(enrichScraperRequest({
    url: 'https://acmebuilders.com',
    strategy: 'cheerio',
    maxEmails: 999,
  }), {
    url: 'https://acmebuilders.com',
    strategy: 'cheerio',
    includeContacts: true,
    includeEmails: true,
    includePhones: true,
    maxEmails: 50,
    maxPhones: 50,
  });
});

test('structured contacts are projected into bounded HTML and text for the existing parser', () => {
  const body = augmentScraperBody({
    ok: true,
    extraction: {
      html: '<html><body>Acme</body></html>',
      text: 'Acme',
      contacts: {
        emails: [{ address: 'hello@acmebuilders.com' }],
        phones: [{ e164: '+14155551212' }],
      },
    },
  });
  assert.match(body.extraction.html, /mailto:hello@acmebuilders\.com/);
  assert.match(body.extraction.html, /tel:\+14155551212/);
  assert.match(body.extraction.text, /hello@acmebuilders\.com/);
  assert.match(body.extraction.text, /\+14155551212/);
});

test('static no-contact results escalate once to Playwright and return rendered contacts', async () => {
  const calls = [];
  const fetchImpl = createScraperContactFetch(async (_input, init) => {
    const payload = JSON.parse(init.body);
    calls.push(payload);
    if (payload.strategy === 'playwright') {
      return response({
        ok: true,
        strategy: 'playwright',
        extraction: {
          html: '<html><body>Rendered</body></html>',
          text: 'Rendered',
          contacts: {
            emails: [{ address: 'info@acmebuilders.com' }],
            phones: [{ e164: '+14155551212' }],
          },
        },
      });
    }
    return response({
      ok: true,
      strategy: 'cheerio',
      extraction: { html: '<html><body>Static</body></html>', text: 'Static', contacts: {} },
    });
  }, { scraperUrl });

  const result = await fetchImpl(new URL('/scrape', scraperUrl), {
    method: 'POST',
    body: JSON.stringify({ url: 'https://acmebuilders.com', strategy: 'cheerio' }),
  });
  const body = await result.json();
  assert.equal(calls.length, 2);
  assert.equal(calls[0].includeContacts, true);
  assert.equal(calls[1].strategy, 'playwright');
  assert.match(body.extraction.html, /mailto:info@acmebuilders\.com/);
  assert.match(body.extraction.html, /tel:\+14155551212/);
});

test('browser and non-scraper requests do not recursively escalate', async () => {
  assert.equal(shouldEscalateToBrowser(
    { strategy: 'playwright' },
    { ok: true, extraction: { html: '<html></html>', contacts: {} } },
  ), false);

  let calls = 0;
  const fetchImpl = createScraperContactFetch(async () => {
    calls += 1;
    return new Response('ok');
  }, { scraperUrl });
  const result = await fetchImpl('https://api.search.brave.com/res/v1/web/search');
  assert.equal(await result.text(), 'ok');
  assert.equal(calls, 1);
});

test('container startup preloads only the contact compatibility bridge', () => {
  const dockerfile = readFileSync(new URL('./Dockerfile', import.meta.url), 'utf8');
  const preload = readFileSync(new URL('./scraper-contact-preload.mjs', import.meta.url), 'utf8');
  assert.match(dockerfile, /NODE_OPTIONS=--import=\/work\/scraper-contact-preload\.mjs/);
  assert.match(preload, /installScraperContactBridge/);
  assert.doesNotMatch(preload, /mail\/send|messages\.send|twilio|hubspot/i);
});
