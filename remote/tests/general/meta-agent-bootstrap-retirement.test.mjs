import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readdir, readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function repositoryRoot() {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '../..')]) {
    if (existsSync(resolve(candidate, '.github/workflows'))) {
      return candidate;
    }
  }
  throw new Error(`unable to locate repository root from ${process.cwd()}`);
}

const root = repositoryRoot();
const retiredWorkflow = resolve(
  root,
  '.github/workflows/ops-owner-device-create-meta-agent-repo.yml',
);
const completionDocument = resolve(
  root,
  'docs/operations/meta-agent-repository-publication-completion.md',
);
const assetsDirectory = resolve(root, 'scripts/critical-org-fleet/assets');
const publisher = resolve(
  root,
  'scripts/critical-org-fleet/publish_meta_control_plane.py',
);
const snapshotVerifier = resolve(
  root,
  'scripts/ops/verify_meta_agent_source_snapshot.py',
);
const ephemeralRunbook = resolve(
  root,
  'docs/operations/meta-agent-ephemeral-credential-publication.md',
);

const read = (path) => readFile(path, 'utf8');

test('completed interactive Meta owner bootstrap is absent from the active workflow surface', async () => {
  assert.equal(existsSync(retiredWorkflow), false);

  const workflowDirectory = resolve(root, '.github/workflows');
  const workflowNames = (await readdir(workflowDirectory)).filter((name) =>
    /\.ya?ml$/.test(name),
  );
  for (const name of workflowNames) {
    const content = await read(resolve(workflowDirectory, name));
    const isMetaOwnerDeviceFlow =
      content.includes('meta-agents-demo/meta-agent-control-plane.rs') &&
      content.includes('https://github.com/login/device/code');
    assert.equal(
      isMetaOwnerDeviceFlow,
      false,
      `${name} restores the retired Meta repository owner-device flow`,
    );
  }
});

test('completion record pins the exact target and initial reviewed history', async () => {
  const document = await read(completionDocument);
  for (const contract of [
    'Status: completed on 2026-08-03 UTC and reverified on 2026-08-04 UTC.',
    '`meta-agents-demo/meta-agent-control-plane.rs`',
    '`4d6ec3ad0ec7b688f0e777129eee7e0f0d999df1`',
    '`789d48039da232faed985d4f8de176959f117e08`',
    'Repository creation is no longer an active cluster operation',
    'must not be restored',
    'never use force pushes',
    'must be revoked and rotated',
    'DEN-1057, DEN-1058, and DEN-319',
  ]) {
    assert.match(document, new RegExp(contract.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));
  }
});

test('immutable recovery evidence remains present after privileged workflow retirement', async () => {
  assert.equal(existsSync(publisher), true);
  assert.equal(existsSync(snapshotVerifier), true);
  assert.equal(existsSync(ephemeralRunbook), true);

  const assets = (await readdir(assetsDirectory))
    .filter((name) => /^meta\.part[^/]+$/.test(name))
    .sort();
  assert.equal(assets.length, 9);

  const document = await read(completionDocument);
  assert.match(
    document,
    /1ddaa03743b864348162149b7d2d2e2dce7eab585cf092ea14547c647fcec031/,
  );
  assert.match(
    document,
    /e2fe6eaa622db02a54f83e27a822f64ad4b54971c883f97bbda4ac0a4db5d278/,
  );
});
