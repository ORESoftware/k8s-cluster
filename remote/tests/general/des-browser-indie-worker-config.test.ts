import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const root = join(import.meta.dirname, '../../..');
const read = (path: string) => readFileSync(join(root, path), 'utf8');

const continuityPatch =
  'remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml';
const profilesPath = 'remote/deployments/build-server-rs/src/profiles.rs';
const plannerPath = 'remote/deployments/build-server-rs/src/gha_workflow.rs';

test('DES browser repos are narrowly admitted to fixed indie profiles', () => {
  const manifest = read(continuityPatch);
  for (const repository of [
    'discrete-event-systems-test/des-web-playwright-e2e',
    'discrete-event-systems-test/des-web-puppeteer-e2e',
  ]) {
    assert.match(manifest, new RegExp(repository.replaceAll('/', '\\/')));
  }
  assert.doesNotMatch(
    manifest,
    /https:\/\/github\.com\/discrete-event-systems-test\/(?:,|\s|$)/,
    'the whole test organization must not be admitted',
  );
});

test('Playwright and Puppeteer profiles remain compiled and execution stays fail-closed', () => {
  const manifest = read(continuityPatch);
  const profiles = read(profilesPath);
  const planner = read(plannerPath);

  assert.match(manifest, /BUILD_SERVER_ALLOWED_PROFILES[\s\S]*playwright,puppeteer/);
  assert.match(
    manifest,
    /BUILD_SERVER_GHA_WORKFLOW_EXECUTION_ENABLED\s+value:\s*'false'/,
  );
  assert.match(profiles, /name:\s*"playwright"/);
  assert.match(profiles, /name:\s*"puppeteer"/);
  assert.match(planner, /revision is not an exact 40-hex commit SHA/);
  assert.match(planner, /workflow execution is disabled/);
});
