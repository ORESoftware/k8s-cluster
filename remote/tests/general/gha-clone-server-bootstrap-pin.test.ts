import { spawnSync } from 'node:child_process';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const reviewedRevision = '412f03155ba108890735414d6fbf5a1a72d9c554';
const root = join(import.meta.dirname, '../../..');
const deployment = readFileSync(
  join(
    root,
    'remote/argocd/dd-next-runtime/dd-gha-clone-server.deployment.yaml',
  ),
  'utf8',
);

const bootstrapBlock = deployment.match(
  /          args:\n            - \|\n([\s\S]*?)\n          env:/,
)?.[1];
assert.ok(bootstrapBlock, 'deployment must contain one literal bootstrap shell');

const bootstrapIndent = '              ';
const bootstrapShell = bootstrapBlock
  .split('\n')
  .map((line) =>
    line.startsWith(bootstrapIndent) ? line.slice(bootstrapIndent.length) : line,
  )
  .join('\n');

test('clone-server source bootstrap is pinned to one exact commit', () => {
  const revision = deployment.match(
    /name:\s*GHA_CLONE_SOURCE_REVISION\s+value:\s*([0-9a-f]{40})/,
  )?.[1];

  assert.equal(
    revision,
    reviewedRevision,
    'the reviewed bootstrap revision must remain explicit and immutable',
  );
  assert.doesNotMatch(deployment, /GHA_CLONE_SOURCE_REF/);
  assert.doesNotMatch(deployment, /clone[^\n]*--branch|value:\s*(?:main|dev)\b/);
  assert.match(
    deployment,
    /fetch --quiet --depth=1 --no-tags origin "\$source_revision"/,
  );
  assert.match(deployment, /checkout --quiet --detach FETCH_HEAD/);
  assert.match(deployment, /rev-parse HEAD/);
  assert.match(deployment, /resolved_revision" != "\$source_revision"/);
  assert.match(deployment, /\breplicas:\s*0\b/);
  assert.match(
    deployment,
    /name:\s*GHA_CLONE_EXECUTION_ENABLED\s+value:\s*"false"/,
  );
});

test('embedded bootstrap shell is syntactically valid', () => {
  const result = spawnSync('bash', ['-n'], {
    input: bootstrapShell,
    encoding: 'utf8',
  });
  assert.equal(result.status, 0, result.stderr);
});

test('revision validation rejects mutable or malformed refs before network I/O', () => {
  const sourceRootOffset = bootstrapShell.indexOf('source_root=');
  assert.notEqual(sourceRootOffset, -1);
  const validationOnly = `${bootstrapShell.slice(0, sourceRootOffset)}exit 0\n`;

  for (const invalid of [
    '',
    'main',
    'dev',
    'ABCDEF0123456789ABCDEF0123456789ABCDEF01',
    '412f03155ba108890735414d6fbf5a1a72d9c55',
    `${reviewedRevision}0`,
    '412f03155ba108890735414d6fbf5a1a72d9c55g',
  ]) {
    const result = spawnSync('bash', ['-c', validationOnly], {
      env: { ...process.env, GHA_CLONE_SOURCE_REVISION: invalid },
      encoding: 'utf8',
    });
    assert.equal(
      result.status,
      64,
      `invalid revision ${JSON.stringify(invalid)} must fail before fetch`,
    );
  }

  const valid = spawnSync('bash', ['-c', validationOnly], {
    env: { ...process.env, GHA_CLONE_SOURCE_REVISION: reviewedRevision },
    encoding: 'utf8',
  });
  assert.equal(valid.status, 0, valid.stderr);
});

test('source bootstrap remains activation-blocked until the runtime is digest-pinned', () => {
  assert.match(
    deployment,
    /image:\s*docker\.io\/library\/rust:1\.90-bookworm/,
  );
  assert.doesNotMatch(deployment, /image:\s*\S+@sha256:[0-9a-f]{64}/);
  assert.match(deployment, /prebuilt digest-pinned image/);
});
