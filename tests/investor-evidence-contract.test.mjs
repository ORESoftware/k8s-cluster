import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import assert from 'node:assert/strict';

const investors = await readFile(new URL('../src/pages/investors.astro', import.meta.url), 'utf8');
const facts = JSON.parse(await readFile(new URL('../public/company-facts.json', import.meta.url), 'utf8'));

const verified = {
  company: 'Fiducia Cloud',
  contact: 'hello@fiducia.cloud',
  website: 'https://fiducia.cloud',
  github: 'https://github.com/fiducia-cloud',
  linkedin: 'https://linkedin.com/in/alexanderdmills',
};

test('HTML and JSON keep the same official company identity', () => {
  assert.equal(facts.company, verified.company);
  assert.equal(facts.official_contact, verified.contact);
  assert.equal(facts.website, verified.website);
  assert.equal(facts.github_organization, verified.github);
  assert.equal(facts.founder.linkedin, verified.linkedin);
  for (const value of Object.values(verified)) assert.ok(investors.includes(value), `investor page missing ${value}`);
});

test('facts expose only bounded public evidence', () => {
  assert.equal(facts.schema_version, 1);
  assert.equal(facts.stage, 'bootstrapped-active-public-implementation');
  assert.match(facts.disclaimer, /excludes private financial, customer, account, credential, and application data/i);
  assert.deepEqual(Object.keys(facts).sort(), [
    'architecture', 'company', 'disclaimer', 'founder', 'github_organization', 'official_contact',
    'positioning', 'primitives', 'public_evidence', 'schema_version', 'stage', 'website',
  ]);
});

test('public evidence does not invent mutable eligibility or traction facts', () => {
  const publicText = `${investors}\n${JSON.stringify(facts)}`;
  for (const forbidden of [
    /\bARR\b/i,
    /annual recurring revenue/i,
    /\brevenue\s*[:=]/i,
    /paying customers?/i,
    /customer count/i,
    /raised \$|funding round|series [a-z]/i,
    /incorporated in|legal entity|headquartered in/i,
    /approved credits?|credit award/i,
    /application count|conference applications?/i,
  ]) {
    assert.doesNotMatch(publicText, forbidden);
  }
});

test('coordination positioning retains safety qualifiers', () => {
  for (const primitive of ['ttl-leases', 'fencing-tokens', 'semaphores', 'leader-election', 'strongly-consistent-work-claiming']) {
    assert.ok(facts.primitives.includes(primitive));
  }
  assert.ok(facts.architecture.includes('sharded-raft-groups'));
  assert.match(investors, /Raft-replicated/);
  assert.doesNotMatch(investors, /\bBFT\b|Byzantine fault tolerant|trustless/i);
});
