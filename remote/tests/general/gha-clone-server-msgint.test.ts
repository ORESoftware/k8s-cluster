import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const root = join(import.meta.dirname, '../../..');
const read = (path: string) => readFileSync(join(root, path), 'utf8');

const configMapPath =
  'remote/argocd/dd-next-runtime/dd-gha-clone-server.configmap.yaml';
const continuityPatchPath =
  'remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml';
const admissionDocPath = 'docs/gha-profile-repository-admission.md';
const profilesPath = 'remote/deployments/build-server-rs/src/profiles.rs';
const plannerPath = 'remote/deployments/gha-clone-server-rs/src/lib.rs';
const fixturePath =
  'remote/deployments/gha-clone-server-rs/tests/fixtures/msgint-operator-config.yml';
const integrationPath =
  'remote/deployments/gha-clone-server-rs/tests/msgint_operator_config.rs';
const workflowPath = '.github/workflows/gha-clone-server.yml';

const connectorRevision = '952623b07fd83caa3a83ee27bdea293f6bd4372f';

test('Messaging Intel admission is exact at both orchestration boundaries', () => {
  const config = read(configMapPath);
  assert.match(config, /messaging-intel\/msgint-connectors/);
  assert.match(
    config,
    /"messaging-intel\/msgint-connectors": \["\.github\/workflows\/gha-clone-operator-config\.yml"\]/,
  );
  assert.doesNotMatch(
    config,
    /"messaging-intel\/msgint-connectors": \["\.github\/workflows\/[^g][^"]*"\]/,
  );

  const patch = read(continuityPatchPath);
  assert.match(
    patch,
    /=https:\/\/github\.com\/messaging-intel\/msgint-connectors\.git/,
  );
  assert.doesNotMatch(
    patch,
    /https:\/\/github\.com\/messaging-intel\/(?!msgint-connectors\.git)/,
  );

  const admission = read(admissionDocPath);
  assert.match(
    admission,
    /=https:\/\/github\.com\/messaging-intel\/msgint-connectors\.git/,
  );
  assert.match(admission, /suffix-appended lookalikes/);
});

test('fixed Node profiles cover operator verification and lifecycle-safe repository tests', () => {
  const profiles = read(profilesPath);
  assert.match(profiles, /name: "node-hardened-verify"/);
  assert.match(profiles, /name: "node-hardened-test"/);
  assert.match(profiles, /npm ci --ignore-scripts/);
  assert.match(profiles, /npm run check/);
  assert.match(profiles, /npm run test:operator-config/);
  assert.match(profiles, /npm audit --audit-level=high/);
  assert.match(profiles, /npm test/);

  const planner = read(plannerPath);
  assert.match(planner, /node-hardened-verify/);
  assert.match(planner, /node-hardened-test/);
  assert.match(planner, /exact reviewed command sequence/);
  assert.match(planner, /exact 40-hex commit SHA/);
  assert.match(planner, /secret-bearing setup inputs are unsupported/);
  assert.match(planner, /secret-bearing step environments are unsupported/);
});

test('bounded Messaging Intel fixture and real-server integration stay adversarial', () => {
  const fixture = read(fixturePath);
  assert.match(fixture, /workflow_dispatch:/);
  assert.match(fixture, /operator_config:/);
  assert.match(fixture, /repository_tests:/);
  assert.match(fixture, /needs: operator_config/);
  assert.equal((fixture.match(/npm ci --ignore-scripts/g) ?? []).length, 2);
  assert.match(fixture, /npm run test:operator-config/);
  assert.match(fixture, /npm audit --audit-level=high/);
  assert.match(fixture, /npm test/);
  assert.equal((fixture.match(/persist-credentials:\s*false/g) ?? []).length, 2);
  assert.doesNotMatch(
    fixture,
    /\$\{\{|secrets\.|working-directory:|timeout-minutes:|permissions:|concurrency:/,
  );
  assert.doesNotMatch(fixture, /services:|container:|strategy:|\bcurl\b|\bwget\b/);

  const integration = read(integrationPath);
  assert.match(integration, /CARGO_BIN_EXE_gha-clone-server/);
  assert.match(integration, new RegExp(connectorRevision));
  assert.match(integration, /submissions\.len\(\), 2/);
  assert.match(integration, /UNPROCESSABLE_ENTITY/);
  assert.match(integration, /npm publish/);
  assert.match(integration, /PROD_TOKEN/);
  assert.match(integration, /dispatched a build despite rejection/);
});

test('manual private smoke uses a scoped GitHub App token and immutable connector head', () => {
  const workflow = read(workflowPath);
  assert.match(workflow, /run_msgint_profile_smoke/);
  assert.match(workflow, /create-github-app-token@/);
  assert.match(workflow, /owner: messaging-intel/);
  assert.match(workflow, /repositories: msgint-connectors/);
  assert.match(workflow, /permission-contents: read/);
  assert.match(workflow, new RegExp(`MSGINT_REVISION: ${connectorRevision}`));
  assert.match(workflow, /NODE_HARDENED_VERIFY_STEPS/);
  assert.match(workflow, /NODE_HARDENED_TEST_STEPS/);
  assert.match(workflow, /node-hardened-verify node-hardened-test/);
  assert.match(workflow, /persist-credentials:\s*false/);
  assert.match(workflow, /--cap-drop=ALL/);
  assert.match(workflow, /--security-opt=no-new-privileges/);
  assert.match(workflow, /--read-only/);
});
