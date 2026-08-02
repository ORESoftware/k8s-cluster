import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import assert from 'node:assert/strict';

const investors = await readFile(new URL('../src/pages/investors.astro', import.meta.url), 'utf8');
const layout = await readFile(new URL('../src/layouts/Layout.astro', import.meta.url), 'utf8');
const facts = JSON.parse(await readFile(new URL('../public/company-facts.json', import.meta.url), 'utf8'));
const renderedSources = `${investors}\n${layout}`;

const verified = Object.freeze({
  company: 'Fiducia Cloud',
  contact: 'hello@fiducia.cloud',
  website: 'https://fiducia.cloud',
  github: 'https://github.com/fiducia-cloud',
  linkedin: 'https://linkedin.com/in/alexanderdmills',
});

const expectedPrimitives = Object.freeze([
  'distributed-locks',
  'ttl-leases',
  'fencing-tokens',
  'semaphores',
  'leader-election',
  'service-discovery',
  'strongly-consistent-work-claiming',
  'rate-limiting',
  'shared-configuration',
  'fail-safe-scheduling',
]);

const primitiveCopy = new Map([
  ['distributed-locks', /Locks & leases/i],
  ['ttl-leases', /TTL expiry/i],
  ['fencing-tokens', /fencing tokens/i],
  ['semaphores', /Semaphores & limits/i],
  ['leader-election', /Leader election/i],
  ['service-discovery', /service discovery/i],
  ['strongly-consistent-work-claiming', /strongly consistent work claiming/i],
  ['rate-limiting', /rate limiting/i],
  ['shared-configuration', /shared configuration/i],
  ['fail-safe-scheduling', /Fail-safe scheduling/i],
]);

const forbiddenMutableClaims = [
  /\b(?:ARR|MRR)\b/i,
  /\b(?:annual|monthly) recurring revenue\b/i,
  /\brevenue\b[^.\n]{0,48}(?:\$|USD|\d|million|thousand)/i,
  /(?:\$|USD)\s?\d[^.\n]{0,48}\b(?:revenue|valuation|funding|credits?)\b/i,
  /\b\d[\d,]*(?:\+)?\s+(?:paying\s+)?customers?\b/i,
  /\b(?:raised|closed|secured)\b[^.\n]{0,48}\b(?:pre-seed|seed|series|funding|capital)\b/i,
  /\b(?:pre-seed|seed|series [a-z])\s+(?:round|funding|backed)\b/i,
  /\b(?:incorporated|headquartered|registered|domiciled|located)\s+in\b/i,
  /\b(?:LLC|C-?Corp(?:oration)?|S-?Corp)\b/i,
  /\b(?:awarded|approved for|received|secured)\b[^.\n]{0,48}\bcredits?\b/i,
  /\b\d[\d,]*(?:\+)?\s+(?:fundraising|computing|AI|conference|speaking)?\s*applications?\b/i,
];

function assertExactKeys(value, expected, label) {
  assert.deepEqual(Object.keys(value).sort(), [...expected].sort(), `${label} exposes an unexpected field`);
}

function assertHttps(url, label) {
  const parsed = new URL(url);
  assert.equal(parsed.protocol, 'https:', `${label} must use HTTPS`);
  assert.equal(parsed.username, '', `${label} must not embed credentials`);
  assert.equal(parsed.password, '', `${label} must not embed credentials`);
  assert.equal(parsed.hash, '', `${label} must not carry a fragment`);
}

function assertNoMutableClaims(text, label) {
  for (const pattern of forbiddenMutableClaims) {
    assert.doesNotMatch(text, pattern, `${label} contains an unverified mutable claim matching ${pattern}`);
  }
}

test('rendered HTML sources and JSON keep one official company identity', () => {
  assert.equal(facts.company, verified.company);
  assert.equal(facts.official_contact, verified.contact);
  assert.equal(facts.website, verified.website);
  assert.equal(facts.github_organization, verified.github);
  assert.equal(facts.founder.name, 'Alexander Mills');
  assert.equal(facts.founder.linkedin, verified.linkedin);

  for (const value of Object.values(verified)) {
    assert.ok(
      renderedSources.includes(value),
      `investor page/layout sources are missing verified identity value: ${value}`,
    );
  }
  assert.match(investors, /Bootstrapped, active public implementation/);
});

test('machine-readable facts expose a closed, bounded public schema', () => {
  assert.equal(facts.schema_version, 1);
  assert.equal(facts.stage, 'bootstrapped-active-public-implementation');
  assert.equal(
    facts.positioning,
    'AI-native coordination and consensus control plane for distributed systems and autonomous-agent fleets',
  );
  assert.match(
    facts.disclaimer,
    /excludes private financial, customer, account, credential, and application data/i,
  );

  assertExactKeys(
    facts,
    [
      'architecture',
      'company',
      'disclaimer',
      'founder',
      'github_organization',
      'official_contact',
      'positioning',
      'primitives',
      'public_evidence',
      'schema_version',
      'stage',
      'website',
    ],
    'company-facts.json',
  );
  assertExactKeys(facts.founder, ['linkedin', 'name'], 'company-facts.json founder');
  assertExactKeys(
    facts.public_evidence,
    ['investor_page', 'source_repositories'],
    'company-facts.json public_evidence',
  );
  assert.deepEqual(facts.primitives, expectedPrimitives);
  assert.deepEqual(facts.architecture, [
    'sharded-raft-groups',
    'quorum-commit',
    'multi-region-control-plane',
    'crash-fault-tolerance-cft-not-byzantine',
  ]);
  assert.equal(facts.public_evidence.investor_page, '/investors');
  assert.equal(facts.public_evidence.source_repositories, verified.github);

  assertHttps(facts.website, 'website');
  assertHttps(facts.github_organization, 'GitHub organization');
  assertHttps(facts.founder.linkedin, 'founder LinkedIn');
  assertHttps(facts.public_evidence.source_repositories, 'source repositories');
  assert.match(facts.official_contact, /^[^\s@]+@[^\s@]+\.[^\s@]+$/);

  const serialized = JSON.stringify(facts);
  assert.doesNotMatch(serialized, /\b(?:null|undefined)\b/);
  assert.doesNotMatch(serialized, /(?:token|secret|password|credential)["']?\s*:/i);
});

test('every machine-readable primitive has visible human-readable evidence', () => {
  for (const primitive of facts.primitives) {
    const matcher = primitiveCopy.get(primitive);
    assert.ok(matcher, `no source-copy contract exists for primitive: ${primitive}`);
    assert.match(investors, matcher, `investor page is missing visible copy for ${primitive}`);
  }
  assert.match(investors, /Sharded Raft groups/i);
  assert.match(investors, /durable majority replication/i);
  assert.match(investors, /multi-region/i);
});

test('public evidence does not invent mutable eligibility, traction, or legal facts', () => {
  assertNoMutableClaims(investors, 'investor page source');
  assertNoMutableClaims(JSON.stringify(facts), 'company-facts.json');
});

test('coordination positioning states its exact crash-fault boundary', () => {
  assert.match(investors, /Raft-replicated/);
  assert.ok(facts.architecture.includes('crash-fault-tolerance-cft-not-byzantine'));
  assert.match(layout, /crash-fault tolerant \(CFT\), not Byzantine/);
  assert.doesNotMatch(investors, /\bBFT\b|Byzantine fault tolerant|trustless/i);
  assert.match(layout, /rel="canonical"/);
  assert.match(layout, /property="og:url"/);
  assert.match(layout, /rel="alternate"/);
});
