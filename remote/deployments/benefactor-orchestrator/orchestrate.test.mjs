import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import {
  ResponseLimitError,
  buildDryRunReport,
  canonicalJson,
  extractEmailsFromText,
  extractPhonesFromText,
  mergeProviderResults,
  normalizeCandidateUrl,
  normalizeEmail,
  normalizePhone,
  normalizeScraperServiceUrl,
  parseBoolean,
  parseBoundedInteger,
  providerStatuses,
  readBodyCapped,
} from './pipeline-lib.mjs';
import { createSearchProviders } from './providers/index.mjs';

const orchestratorSource = readFileSync(new URL('./orchestrate.mjs', import.meta.url), 'utf8');

test('bounded integer configuration rejects NaN, negative, and oversized values', () => {
  assert.equal(parseBoundedInteger('COUNT', undefined, { defaultValue: 8, min: 1, max: 10 }), 8);
  assert.equal(parseBoundedInteger('COUNT', '10', { min: 1, max: 10 }), 10);
  assert.throws(() => parseBoundedInteger('COUNT', '1.5', { min: 1, max: 10 }), /must be an integer/);
  assert.throws(() => parseBoundedInteger('COUNT', '-1', { min: 1, max: 10 }), />= 1/);
  assert.throws(() => parseBoundedInteger('COUNT', '11', { min: 1, max: 10 }), /<= 10/);
});

test('boolean configuration is explicit and fail closed', () => {
  assert.equal(parseBoolean(undefined, true), true);
  assert.equal(parseBoolean('false', true), false);
  assert.equal(parseBoolean('YES', false), true);
  assert.throws(() => parseBoolean('sometimes'), /invalid boolean/);
});

test('candidate URL validation admits public HTTP(S) and rejects SSRF-shaped targets', () => {
  assert.equal(normalizeCandidateUrl('https://www.example-business.com/contact#team'), 'https://www.example-business.com/contact');
  for (const target of [
    'file:///etc/passwd',
    'http://localhost/admin',
    'http://127.0.0.1/',
    'http://169.254.169.254/latest/meta-data/',
    'http://[::1]/',
    'http://service.default.svc.cluster.local/',
    'https://metadata.google.internal/',
    'https://user:pass@example.com/',
    'https://example.com:8443/',
    'https://single-label/',
    'https://xn--bcher-kva.example/',
  ]) {
    assert.throws(() => normalizeCandidateUrl(target), undefined, target);
  }
});

test('scraper credentials can only be sent to an explicitly allowlisted private service', () => {
  assert.equal(
    normalizeScraperServiceUrl('http://dd-web-scraper.default.svc.cluster.local:8097', [
      'dd-web-scraper.default.svc.cluster.local',
    ]),
    'http://dd-web-scraper.default.svc.cluster.local:8097/',
  );
  assert.throws(
    () => normalizeScraperServiceUrl('https://attacker.example.com', [
      'dd-web-scraper.default.svc.cluster.local',
    ]),
    /not in SCRAPER_ALLOWED_HOSTS/,
  );
  assert.throws(
    () => normalizeScraperServiceUrl('http://scraper.example.com', ['scraper.example.com']),
    /plain HTTP scraper traffic/,
  );
  assert.throws(
    () => normalizeScraperServiceUrl('https://user:secret@scraper.example.com', ['scraper.example.com']),
    /must not contain credentials/,
  );
});

test('response reader rejects declared and streaming overflow before unbounded buffering', async () => {
  const declared = new Response('ok', { headers: { 'content-length': '1000' } });
  await assert.rejects(() => readBodyCapped(declared, { maxBytes: 10 }), ResponseLimitError);

  const streaming = new Response(new ReadableStream({
    start(controller) {
      controller.enqueue(new Uint8Array(8));
      controller.enqueue(new Uint8Array(8));
      controller.close();
    },
  }));
  await assert.rejects(() => readBodyCapped(streaming, { maxBytes: 10 }), ResponseLimitError);
  assert.equal(await readBodyCapped(new Response('bounded'), { maxBytes: 20 }), 'bounded');
});

test('response reader enforces a body deadline', async () => {
  const stalled = new Response(new ReadableStream({ start() {} }));
  await assert.rejects(
    () => readBodyCapped(stalled, { maxBytes: 20, timeoutMs: 10 }),
    /timed out/,
  );
});

test('email extraction normalizes obfuscation and rejects consumer or non-role addresses', () => {
  assert.deepEqual(
    extractEmailsFromText('Contact hello [at] AcmeBuilders [dot] com or person@acmebuilders.com'),
    ['hello@acmebuilders.com'],
  );
  assert.equal(normalizeEmail('support@acmebuilders.com'), 'support@acmebuilders.com');
  assert.equal(normalizeEmail('owner@gmail.com'), null);
  assert.equal(normalizeEmail('jane@acmebuilders.com'), null);
  assert.equal(normalizeEmail('jane@acmebuilders.com', { requireRoleEmail: false }), 'jane@acmebuilders.com');
});

test('phone extraction emits bounded E.164 candidates', () => {
  assert.equal(normalizePhone('(415) 555-1212'), '+14155551212');
  assert.equal(normalizePhone('+51 987 654 321'), '+51987654321');
  assert.equal(normalizePhone('123'), null);
  assert.deepEqual(extractPhonesFromText('Call (415) 555-1212 or +51 987 654 321.'), ['+14155551212', '+51987654321']);
});

test('missing credentials disable one provider without disabling configured peers', () => {
  const statuses = providerStatuses({ serperKey: 'configured', braveKey: '' });
  const serper = statuses.find((item) => item.provider === 'serper');
  const brave = statuses.find((item) => item.provider === 'brave');
  assert.deepEqual({ enabled: serper.enabled, status: serper.status }, { enabled: true, status: 'configured' });
  assert.deepEqual(
    { enabled: brave.enabled, status: brave.status },
    { enabled: false, status: 'disabled_missing_credentials' },
  );
});

test('search provider adapters are isolated so one provider failure does not disable its peer', async () => {
  const calls = [];
  const adapters = createSearchProviders({
    serperKey: 'serper-secret',
    braveKey: 'brave-secret',
    async fetchJson(url, init) {
      calls.push({ url: String(url), init });
      if (String(url).includes('serper.dev')) throw new Error('provider unavailable');
      return { web: { results: [{ url: 'https://beta.example-business.com/' }] } };
    },
  });
  const serper = adapters.find((adapter) => adapter.name === 'serper');
  const brave = adapters.find((adapter) => adapter.name === 'brave');
  await assert.rejects(() => serper.search('roofing', 10), /provider unavailable/);
  assert.deepEqual(await brave.search('roofing', 10), ['https://beta.example-business.com/']);
  assert.equal(calls.length, 2);
  assert.equal(serper.configured, true);
  assert.equal(brave.configured, true);
});

test('provider result merge rejects unsafe candidates, dedupes domains, and preserves provenance', () => {
  const results = mergeProviderResults([
    { provider: 'serper', results: ['https://acme.example-business.com/contact', 'http://127.0.0.1/'] },
    { provider: 'brave', results: ['https://acme.example-business.com/about', 'https://beta.example-business.com/'] },
  ]);
  assert.equal(results.length, 2);
  assert.deepEqual(
    results.map(({ domain, provider }) => ({ domain, provider })),
    [
      { domain: 'beta.example-business.com', provider: 'brave' },
      { domain: 'acme.example-business.com', provider: 'serper' },
    ],
  );
});

test('dry-run reports are deterministic, sorted, and exclude raw email identifiers', () => {
  const providers = providerStatuses({ serperKey: 'configured', braveKey: '' });
  const records = [
    {
      email: 'sales@beta.example-business.com',
      domain: 'beta.example-business.com',
      sourceUrl: 'https://beta.example-business.com/',
      provider: 'serper',
      providerRank: 2,
      confidence: 0.85,
      verificationStatus: 'syntax_valid',
      queryId: '2',
      query: 'beta query',
      phones: ['+14155551212'],
    },
    {
      email: 'hello@alpha.example-business.com',
      domain: 'alpha.example-business.com',
      sourceUrl: 'https://alpha.example-business.com/',
      provider: 'serper',
      providerRank: 1,
      confidence: 0.95,
      verificationStatus: 'syntax_valid',
      queryId: '1',
      query: 'alpha query',
      phones: [],
    },
  ];
  const first = buildDryRunReport({ category: 'roofing', providers, records, counters: { visited: 2 } });
  const second = buildDryRunReport({ category: 'roofing', providers: [...providers].reverse(), records: [...records].reverse(), counters: { visited: 2 } });
  assert.equal(canonicalJson(first), canonicalJson(second));
  assert.equal(first.reportDigest, second.reportDigest);
  assert.doesNotMatch(canonicalJson(first), /sales@|hello@/);
  assert.match(first.reportDigest, /^sha256:[0-9a-f]{64}$/);
});

test('source contract keeps arbitrary crawling private, bounded, provider-attributed, and dry-run safe', () => {
  assert.match(orchestratorSource, /scrapeViaPrivateService/);
  assert.doesNotMatch(orchestratorSource, /async function scrapeDirect/);
  assert.doesNotMatch(orchestratorSource, /fetch\(candidateUrl/);
  assert.match(orchestratorSource, /ALLOW_DIRECT_FALLBACK is no longer supported/);
  assert.match(orchestratorSource, /readJsonCapped/);
  assert.match(orchestratorSource, /source_engine, tags, meta_data/);
  assert.match(orchestratorSource, /record\.provider/);
  assert.match(orchestratorSource, /if \(config\.dryRun\) return/);
  assert.match(orchestratorSource, /await db\.query\('begin'\)/);
  assert.match(orchestratorSource, /await db\.query\('commit'\)/);
  assert.match(orchestratorSource, /await db\.query\('rollback'\)/);
  assert.doesNotMatch(orchestratorSource, /mail\/send|messages\.send|twilio/i);
});
