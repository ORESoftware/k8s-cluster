import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';

const manifestPath = path.resolve('dist', 'deployment.json');
const deployment = JSON.parse(readFileSync(manifestPath, 'utf8'));

const allowedKeys = [
  'buildEnvironment',
  'commitSha',
  'refName',
  'repository',
  'schemaVersion',
  'sourceUrl',
];
assert.deepEqual(
  Object.keys(deployment).sort(),
  allowedKeys,
  'deployment.json must expose only the reviewed provenance fields',
);
assert.equal(deployment.schemaVersion, 1);

const expectedRepository = process.env.GITHUB_REPOSITORY?.trim() ||
  'sonus-auris/sonus-auris-site.web';
assert.equal(deployment.repository, expectedRepository);
assert.match(
  deployment.repository,
  /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/,
  'deployment repository must be an owner/name identifier',
);

const rawCommitSha = process.env.GITHUB_SHA?.trim() || '';
const expectedCommitSha = /^[0-9a-f]{40}$/i.test(rawCommitSha)
  ? rawCommitSha.toLowerCase()
  : null;
assert.equal(deployment.commitSha, expectedCommitSha);
assert.equal(
  deployment.buildEnvironment,
  expectedCommitSha ? 'github-actions' : 'local',
);

const expectedRefName = process.env.GITHUB_REF_NAME?.trim() ||
  (expectedCommitSha ? 'unknown' : 'local');
assert.equal(deployment.refName, expectedRefName);

if (expectedCommitSha) {
  assert.equal(
    deployment.sourceUrl,
    `https://github.com/${expectedRepository}/commit/${expectedCommitSha}`,
  );
} else {
  assert.equal(deployment.sourceUrl, null);
}

const serialized = JSON.stringify(deployment);
for (const forbidden of [
  'authorization',
  'cookie',
  'password',
  'private_key',
  'secret',
  'token',
]) {
  assert.ok(
    !serialized.toLowerCase().includes(forbidden),
    `deployment.json must not expose ${forbidden}-like data`,
  );
}

console.log(
  `Verified deployment provenance for ${deployment.repository}@${deployment.commitSha ?? 'local'}.`,
);
