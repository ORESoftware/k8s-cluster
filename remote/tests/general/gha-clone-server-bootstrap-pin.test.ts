import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const root = join(import.meta.dirname, '../../..');
const deployment = readFileSync(
  join(
    root,
    'remote/argocd/dd-next-runtime/dd-gha-clone-server.deployment.yaml',
  ),
  'utf8',
);

test('clone-server source bootstrap is pinned to one exact commit', () => {
  const revision = deployment.match(
    /name:\s*GHA_CLONE_SOURCE_REVISION\s+value:\s*([0-9a-f]{40})/,
  )?.[1];

  assert.equal(
    revision,
    '412f03155ba108890735414d6fbf5a1a72d9c554',
    'the reviewed bootstrap revision must remain explicit and immutable',
  );
  assert.doesNotMatch(deployment, /GHA_CLONE_SOURCE_REF/);
  assert.doesNotMatch(deployment, /clone[^\n]*--branch|value:\s*(?:main|dev)\b/);
  assert.match(deployment, /fetch --quiet --depth=1 --no-tags origin "\$source_revision"/);
  assert.match(deployment, /checkout --quiet --detach FETCH_HEAD/);
  assert.match(deployment, /rev-parse HEAD/);
  assert.match(deployment, /resolved_revision" != "\$source_revision"/);
  assert.match(deployment, /\breplicas:\s*0\b/);
  assert.match(
    deployment,
    /name:\s*GHA_CLONE_EXECUTION_ENABLED\s+value:\s*"false"/,
  );
});

test('source bootstrap remains activation-blocked until the runtime is digest-pinned', () => {
  assert.match(deployment, /image:\s*docker\.io\/library\/rust:1\.90-bookworm/);
  assert.doesNotMatch(deployment, /image:\s*\S+@sha256:[0-9a-f]{64}/);
  assert.match(deployment, /prebuilt digest-pinned image/);
});
