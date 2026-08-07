import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const root = join(import.meta.dirname, '../../..');
const read = (path: string) => readFileSync(join(root, path), 'utf8');

const config = read(
  'remote/argocd/dd-next-runtime/dd-gha-clone-server.configmap.yaml',
);
const buildPatch = read(
  'remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml',
);
const workflow = read('.github/workflows/gha-clone-server.yml');
const planner = read('remote/deployments/gha-clone-server-rs/src/lib.rs');
const fixture = read(
  'remote/deployments/gha-clone-server-rs/tests/fixtures/msgint-operator-config.yml',
);
const processTest = read(
  'remote/deployments/gha-clone-server-rs/tests/msgint_operator_config.rs',
);
const profiles = read('remote/deployments/build-server-rs/src/profiles.rs');
const validation = read('remote/deployments/build-server-rs/src/validation.rs');
const admissionDoc = read('docs/gha-profile-repository-admission.md');

const repository = 'messaging-intel/msgint-connectors';
const workflowPath = '.github/workflows/gha-clone-operator-config.yml';
const revision = '952623b07fd83caa3a83ee27bdea293f6bd4372f';

test('Messaging Intel repository and workflow admission are exact and additive', () => {
  assert.match(config, /messaging-intel\/msgint-connectors/);
  assert.match(
    config,
    /"messaging-intel\/msgint-connectors": \["\.github\/workflows\/gha-clone-operator-config\.yml"\]/,
  );
  assert.match(config, /ORESoftware\/k8s-cluster/);
  assert.match(config, /sonus-auris\/sonus-auris-interfaces/);
  assert.doesNotMatch(config, /messaging-intel\/\*|msgint-connectors\/\*/);

  assert.match(
    buildPatch,
    /=https:\/\/github\.com\/messaging-intel\/msgint-connectors\.git/,
  );
  assert.match(
    admissionDoc,
    /=https:\/\/github\.com\/messaging-intel\/msgint-connectors\.git/,
  );
  assert.match(validation, /Exact\(/);
  assert.match(validation, /suffix-appended/);
});

test('bounded workflow uses immutable setup actions and exact reviewed commands', () => {
  assert.match(
    fixture,
    /actions\/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1/,
  );
  assert.match(
    fixture,
    /actions\/setup-node@820762786026740c76f36085b0efc47a31fe5020/,
  );
  assert.match(
    fixture,
    /npm ci --ignore-scripts[\s\S]*npm run check[\s\S]*npm run test:operator-config[\s\S]*npm audit --audit-level=high/,
  );
  assert.match(
    fixture,
    /repository_tests:[\s\S]*needs: operator_config[\s\S]*npm ci --ignore-scripts[\s\S]*npm test/,
  );
  assert.doesNotMatch(fixture, /\benv:\s|\$\{\{\s*secrets|npm publish|pull_request_target/);

  assert.match(planner, /node-hardened-verify/);
  assert.match(planner, /node-hardened-test/);
  assert.match(planner, /exact reviewed command sequence/);
  assert.match(planner, /exact 40-hex commit SHA/);
  assert.match(planner, /fixed profiles do not forward caller-selected variables/);
});

test('build profiles are lifecycle-script-free and use fixed reviewed names', () => {
  for (const profile of ['node-hardened-verify', 'node-hardened-test']) {
    assert.match(profiles, new RegExp(`name: "${profile}"`));
  }
  assert.match(profiles, /npm ci --ignore-scripts/);
  assert.match(profiles, /npm run test:operator-config/);
  assert.match(profiles, /npm audit --audit-level=high/);
  assert.doesNotMatch(profiles, /npm ci(?! --ignore-scripts)/);
});

test('real-process proof and manual private smoke share one immutable revision', () => {
  assert.ok(processTest.includes(`const REVISION: &str = "${revision}"`));
  assert.ok(processTest.includes(`const REPOSITORY: &str = "${repository}"`));
  assert.ok(processTest.includes(`const WORKFLOW_PATH: &str = "${workflowPath}"`));
  assert.match(processTest, /node-hardened-verify/);
  assert.match(processTest, /node-hardened-test/);
  assert.match(processTest, /HTTP 422|UNPROCESSABLE_ENTITY/);
  assert.match(processTest, /zero|submissions.*2/s);

  assert.match(workflow, new RegExp(`MSGINT_REVISION: ${revision}`));
  assert.match(workflow, /actions\/create-github-app-token@fee1f7d63c2ff003460e3d139729b119787bc349/);
  assert.match(workflow, /owner: messaging-intel/);
  assert.match(workflow, /repositories: msgint-connectors/);
  assert.match(workflow, /permission-contents: read/);
  assert.match(workflow, /persist-credentials: false/);
  assert.match(workflow, /run_msgint_profile_smoke/);
  assert.match(workflow, /github\.event_name == 'workflow_dispatch'/);
});

test('normal validation stays credential free and preserves newer continuity gates', () => {
  assert.match(workflow, /Test build request idempotency and retry semantics/);
  assert.match(workflow, /Test strict NATS conflict and redelivery classification/);
  assert.match(workflow, /executor_router_http/);
  assert.match(workflow, /gha-clone-webhook-config\.test\.ts/);
  assert.match(workflow, /gha-clone-msgint-config\.test\.ts/);
  assert.match(workflow, /msgint-operator-config\.yml/);
  assert.match(workflow, /node-hardened-profile/);
  assert.doesNotMatch(
    [workflow, config, buildPatch, fixture, processTest].join('\n'),
    /ghp_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY/,
  );
});
