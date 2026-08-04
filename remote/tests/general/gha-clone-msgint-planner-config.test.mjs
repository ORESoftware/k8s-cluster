import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import test from 'node:test';

const root = join(import.meta.dirname, '../../..');
const read = (path) => readFileSync(join(root, path), 'utf8');

const config = read('remote/argocd/dd-next-runtime/dd-gha-clone-server.configmap.yaml');
const wrapper = read('remote/deployments/gha-clone-server-rs/src/bounded_lib.rs');
const contract = read('remote/deployments/gha-clone-server-rs/src/msgint_contract.rs');
const fixture = read(
  'remote/deployments/gha-clone-server-rs/fixtures/msgint-operator-config.yml',
);
const plannerTest = read(
  'remote/deployments/gha-clone-server-rs/tests/msgint_planner.rs',
);
const documentation = read('docs/gha-msgint-exact-planner.md');
const workflow = read('.github/workflows/gha-clone-server.yml');

test('GitOps grants only the exact Messaging Intel repository and workflow path', () => {
  assert.match(config, /messaging-intel\/msgint-connectors/);
  assert.match(config, /"messaging-intel\/msgint-connectors": \["\.github\/workflows\/gha-clone-operator-config\.yml"\]/);
  assert.doesNotMatch(config, /messaging-intel\/\*/);
  assert.doesNotMatch(config, /messaging-intel\/other/);
  assert.doesNotMatch(config, /"messaging-intel\/msgint-connectors": \[[^\]]*ci\.yml/);
});

test('reserved contract runs before generic and 3FA classifiers', () => {
  const contractCall = wrapper.indexOf('classify_msgint_workflow(');
  const threefaGate = wrapper.indexOf('is_threefa_bounded_workflow(request)');
  assert.ok(contractCall >= 0, 'Messaging Intel contract call is missing');
  assert.ok(threefaGate > contractCall, 'reserved contract must run before the 3FA/generic return path');
  assert.match(wrapper, /ContractMatch::Match/);
  assert.match(wrapper, /ContractMatch::Reject/);
  assert.match(wrapper, /reject_reserved_plan/);
  assert.match(wrapper, /apply_reserved_profiles/);
  assert.match(wrapper, /is_msgint_reserved/);
});

test('contract pins exact identity, revision, workflow, actions, inputs, DAG, and commands', () => {
  for (const marker of [
    'messaging-intel/msgint-connectors',
    'a9cc977d78347ec0efdbe8e6766967f80d425882',
    '.github/workflows/gha-clone-operator-config.yml',
    'Messaging Intel GHA clone operator verification',
    'actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1',
    'actions/setup-node@820762786026740c76f36085b0efc47a31fe5020',
    '22.23.1',
    'persist-credentials',
    'operator_config',
    'repository_tests',
    'node-hardened-verify',
    'node-hardened-test',
    'npm ci --ignore-scripts',
    'npm run test:operator-config',
    'npm audit --audit-level=high',
  ]) {
    assert.ok(contract.includes(marker), `contract is missing ${marker}`);
    assert.ok(fixture.includes(marker) || marker.startsWith('node-hardened-'), `fixture is missing ${marker}`);
  }
  assert.match(contract, /trigger must be exactly workflow_dispatch without inputs/);
  assert.match(contract, /job set or order differs from the reviewed two-job DAG/);
  assert.match(contract, /exact reviewed command sequence differs or contains extra commands/);
  assert.match(contract, /contains_secret_expression/);
});

test('planner tests cover terminal identity and structural lookalikes', () => {
  assert.match(plannerTest, /reserved_repository_path_and_revision_mismatches_are_terminal/);
  assert.match(plannerTest, /lookalike\/msgint-connectors/);
  assert.match(plannerTest, /0000000000000000000000000000000000000000/);
  assert.match(plannerTest, /permissions: read-all/);
  assert.match(plannerTest, /actions\/checkout@main/);
  assert.match(plannerTest, /persist-credentials: true/);
  assert.match(plannerTest, /node-version: \\"22\\"/);
  assert.match(plannerTest, /registry-url: https:\/\/evil\.invalid/);
  assert.match(plannerTest, /npm publish/);
  assert.match(plannerTest, /secrets\.PROD_TOKEN/);
  assert.match(plannerTest, /shell: bash/);
  assert.match(plannerTest, /unrelated_workflow_retains_legacy_classifier_behavior/);
});

test('fixture is static, secret-free, lifecycle-safe, and non-publishing', () => {
  assert.match(fixture, /workflow_dispatch:/);
  assert.match(fixture, /operator_config:[\s\S]*repository_tests:/);
  assert.match(fixture, /repository_tests:[\s\S]*needs: operator_config/);
  assert.match(fixture, /npm ci --ignore-scripts/);
  assert.doesNotMatch(fixture, /\$\{\{|secrets\.|github\.token|actions_id_token_request/);
  assert.doesNotMatch(fixture, /@(main|master|stable|latest)\b/);
  assert.doesNotMatch(fixture, /npm publish|npm install|\|\| true/);
});

test('documentation and permanent workflow preserve plan-only and inactive-live boundaries', () => {
  assert.match(documentation, /reserved namespace/);
  assert.match(documentation, /cannot fall back to the generic Node classifier/);
  assert.match(documentation, /zero replicas/);
  assert.match(documentation, /separate PR must start the actual binary/);
  assert.match(documentation, /least-privilege GitHub App/);
  assert.match(documentation, /No classic PAT/);
  assert.match(workflow, /remote\/deployments\/gha-clone-server-rs\/\*\*/);
  assert.match(workflow, /remote\/tests\/general\/gha-clone-\*\.test\.(?:ts|mjs)/);
});
