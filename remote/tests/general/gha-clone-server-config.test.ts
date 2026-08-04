import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const root = join(import.meta.dirname, '../../..');
const read = (path: string) => readFileSync(join(root, path), 'utf8');

const deploymentPath =
  'remote/argocd/dd-next-runtime/dd-gha-clone-server.deployment.yaml';
const configMapPath =
  'remote/argocd/dd-next-runtime/dd-gha-clone-server.configmap.yaml';
const secretPath =
  'remote/argocd/dd-next-runtime/dd-gha-clone-server.externalsecret.yaml';
const networkPath =
  'remote/argocd/dd-next-runtime/dd-gha-clone-server.networkpolicy.yaml';
const servicePath =
  'remote/argocd/dd-next-runtime/dd-gha-clone-server.service.yaml';
const kustomizationPath = 'remote/argocd/dd-next-runtime/kustomization.yaml';
const observabilityConfigPath =
  'remote/argocd/observability/k8s-resource-exporter.configmap.yaml';
const observabilityDeploymentPath =
  'remote/argocd/observability/k8s-resource-exporter.deployment.yaml';
const profilesPath = 'remote/deployments/build-server-rs/src/profiles.rs';
const continuityPatchPath =
  'remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml';
const plannerPath = 'remote/deployments/gha-clone-server-rs/src/lib.rs';
const serverPath = 'remote/deployments/gha-clone-server-rs/src/main.rs';
const metaIntegrationTestPath =
  'remote/deployments/gha-clone-server-rs/tests/meta_self_test.rs';
const msgintIntegrationTestPath =
  'remote/deployments/gha-clone-server-rs/tests/msgint_operator_config.rs';
const msgintFixturePath =
  'remote/deployments/gha-clone-server-rs/tests/fixtures/msgint-operator-config.yml';
const workflowPath = '.github/workflows/gha-clone-server.yml';
const metaWorkflowPath = '.github/workflows/gha-clone-server-meta.yml';

test('GHA continuity service is installed fail-closed with no cluster identity', () => {
  const deployment = read(deploymentPath);
  assert.match(deployment, /\breplicas:\s*0\b/);
  assert.match(deployment, /\bautomountServiceAccountToken:\s*false\b/);
  assert.match(
    deployment,
    /name:\s*GHA_CLONE_EXECUTION_ENABLED\s+value:\s*"false"/,
  );
  assert.match(
    deployment,
    /name:\s*GHA_CLONE_WEBHOOK_EXECUTION_ENABLED\s+value:\s*"false"/,
  );
  assert.match(deployment, /capabilities:\s+drop:\s*\["ALL"\]/);
  assert.doesNotMatch(deployment, /hostPath:/);
  assert.doesNotMatch(deployment, /docker\.sock|containerd\.sock|buildkitd\.sock/);
  assert.match(deployment, /name:\s*dd-gha-clone-server-secrets/);
});

test('all service resources participate in the dd-next-runtime render', () => {
  const kustomization = read(kustomizationPath);
  for (const resource of [
    'dd-gha-clone-server.configmap.yaml',
    'dd-gha-clone-server.externalsecret.yaml',
    'dd-gha-clone-server.deployment.yaml',
    'dd-gha-clone-server.service.yaml',
    'dd-gha-clone-server.networkpolicy.yaml',
  ]) {
    assert.ok(
      kustomization.includes(`- ${resource}`),
      `${resource} is missing from dd-next-runtime kustomization`,
    );
  }
  assert.match(read(servicePath), /port:\s*8125/);
});

test('continuity deployment is registered with both resource exporter inventories', () => {
  for (const path of [observabilityConfigPath, observabilityDeploymentPath]) {
    assert.match(
      read(path),
      /dd-build-server,dd-gha-clone-server,/,
      `${path} must watch dd-gha-clone-server immediately after dd-build-server`,
    );
  }
});

test('secret mapping names values without committing credential material', () => {
  const secret = read(secretPath);
  assert.match(secret, /dd\/remote-dev\/gha-clone-server-secrets/);
  for (const property of [
    'auth_secret',
    'github_webhook_secret',
    'github_app_installation_token',
    'build_server_auth',
  ]) {
    assert.match(secret, new RegExp(`property: ${property}`));
  }
  assert.doesNotMatch(
    secret,
    /ghp_[A-Za-z0-9]+|github_pat_[A-Za-z0-9_]+|BEGIN (?:RSA |EC )?PRIVATE KEY/,
  );
});

test('network boundary permits only DNS, GitHub HTTPS, and build-server dispatch', () => {
  const policy = read(networkPath);
  assert.match(policy, /port:\s*53/);
  assert.match(policy, /app:\s*dd-build-server/);
  assert.match(policy, /port:\s*8100/);
  assert.match(policy, /port:\s*443/);
  assert.match(policy, /10\.0\.0\.0\/8/);
  assert.match(policy, /192\.168\.0\.0\/16/);
});

test('config allowlists exact trusted repositories and the bounded meta workflow', () => {
  const config = read(configMapPath);
  assert.match(config, /ORESoftware\/k8s-cluster/);
  assert.match(config, /messaging-intel\/msgint-connectors/);
  assert.match(config, /sonus-auris\/sonus-auris-interfaces/);
  assert.match(config, /\.github\/workflows\/ci\.yml/);
  assert.match(
    config,
    /"ORESoftware\/k8s-cluster": \["\.github\/workflows\/gha-clone-server-meta\.yml"\]/,
  );
  assert.doesNotMatch(
    config,
    /"ORESoftware\/k8s-cluster": \["\.github\/workflows\/gha-clone-server\.yml"\]/,
  );
  assert.match(
    config,
    /"messaging-intel\/msgint-connectors": \["\.github\/workflows\/gha-clone-operator-config\.yml"\]/,
  );
  assert.doesNotMatch(config, /https?:\/\/[^/\s]+\/\*|owner\/\*/);
});

test('build server exposes fixed Rust, Node, and Python continuity profiles', () => {
  const profiles = read(profilesPath);
  for (const profile of [
    'rust-verify',
    'node-verify',
    'node-hardened-verify',
    'python-verify',
  ]) {
    assert.match(profiles, new RegExp(`name: "${profile}"`));
  }
  assert.match(
    profiles,
    /cargo clippy --locked --all-targets --all-features -- -D warnings/,
  );
  assert.match(
    profiles,
    /remote\/deployments\/gha-clone-server-rs\/Cargo\.toml/,
  );
  assert.match(profiles, /pnpm install --frozen-lockfile/);
  assert.match(profiles, /npm ci --ignore-scripts/);
  assert.match(profiles, /npm run test:operator-config/);
  assert.match(profiles, /npm audit --audit-level=high/);
  assert.match(profiles, /python -m pytest/);

  const continuityPatch = read(continuityPatchPath);
  assert.match(continuityPatch, /node-hardened-verify/);
  assert.match(
    continuityPatch,
    /https:\/\/github\.com\/messaging-intel\/msgint-connectors\.git/,
  );
  assert.doesNotMatch(profiles, /find .*Cargo\.toml|for crate in/);

  const imageAssignments = [
    ...profiles.matchAll(/const\s+[A-Z_]+_IMAGE:\s*&str\s*=\s*"([^"]+)";/g),
  ];
  assert.ok(imageAssignments.length >= 5, 'expected all fixed runner image assignments');
  for (const [, image] of imageAssignments) {
    assert.ok(!image.endsWith(':latest'), `runner image must be pinned: ${image}`);
  }
});

test('planner and dispatcher preserve the fail-closed command boundary', () => {
  const planner = read(plannerPath);
  const server = read(serverPath);
  assert.match(planner, /service containers require the isolated ARC DinD lane/);
  assert.match(planner, /secret-bearing env\/with values are unsupported/);
  assert.match(planner, /workflow job dependency graph contains a cycle/);
  assert.match(planner, /revision is not an exact 40-hex commit SHA/);
  assert.match(planner, /workflow-level .* is unsupported by the independent lane/);
  assert.match(planner, /"working-directory"/);
  assert.match(planner, /unsupported by the fixed-profile executor/);
  assert.match(planner, /non-Linux native execution is unavailable/);
  assert.match(planner, /node-hardened-verify/);
  assert.match(server, /job_kind: "run-profile"/);
  assert.match(server, /profile,/);
  assert.doesNotMatch(server, /command:\s*&|script:\s*&|runner_image/);
});

test('meta integration test starts the real server and submits its own workflow', () => {
  const integration = read(metaIntegrationTestPath);
  assert.match(integration, /CARGO_BIN_EXE_gha-clone-server/);
  assert.match(integration, /\/v1\/runs/);
  assert.match(integration, /MockBuildState/);
  assert.match(integration, /gha-clone-server-meta\.yml/);
  assert.match(integration, /"profile"\], "rust-verify"/);
  assert.match(integration, /GHA_CLONE_EXECUTION_ENABLED", "true"/);
  assert.match(integration, /env_remove\("GHA_CLONE_GITHUB_TOKEN"\)/);
});

test('Messaging Intel integration starts the real server and dispatches both fixed profiles', () => {
  const integration = read(msgintIntegrationTestPath);
  assert.match(integration, /CARGO_BIN_EXE_gha-clone-server/);
  assert.match(integration, /messaging-intel\/msgint-connectors/);
  assert.match(integration, /gha-clone-operator-config\.yml/);
  assert.match(integration, /node-hardened-verify/);
  assert.match(integration, /node-verify/);
  assert.match(integration, /submissions\.len\(\), 2/);
  assert.match(integration, /env_remove\("GHA_CLONE_GITHUB_TOKEN"\)/);
});

test('Messaging Intel mirror remains independently compilable and non-secret', () => {
  const workflow = read(msgintFixturePath);
  assert.match(workflow, /workflow_dispatch:/);
  assert.match(workflow, /operator_config:/);
  assert.match(workflow, /repository_tests:/);
  assert.match(workflow, /needs: operator_config/);
  assert.match(workflow, /npm ci --ignore-scripts/);
  assert.match(workflow, /npm run check/);
  assert.match(workflow, /npm run test:operator-config/);
  assert.match(workflow, /npm audit --audit-level=high/);
  assert.match(workflow, /npm test/);
  assert.equal((workflow.match(/persist-credentials:\s*false/g) ?? []).length, 2);
  assert.doesNotMatch(
    workflow,
    /\$\{\{|secrets\.|working-directory:|timeout-minutes:|permissions:|concurrency:/,
  );
  assert.doesNotMatch(workflow, /services:|container:|strategy:|\bcurl\b|\bwget\b/);
});

test('bounded meta workflow remains independently compilable', () => {
  const workflow = read(metaWorkflowPath);
  assert.match(workflow, /workflow_dispatch:/);
  assert.match(workflow, /gha-clone-self-test:/);
  assert.match(
    workflow,
    /cargo test --manifest-path remote\/deployments\/gha-clone-server-rs\/Cargo\.toml --locked --all-targets/,
  );
  assert.match(workflow, /persist-credentials:\s*false/);
  assert.doesNotMatch(workflow, /working-directory:|timeout-minutes:|permissions:/);
  assert.doesNotMatch(workflow, /\$\{\{\s*secrets\./);
  assert.doesNotMatch(workflow, /services:|container:|strategy:|needs:/);
});

test('dedicated GitHub Actions workflow checks Rust and deployment contracts', () => {
  const workflow = read(workflowPath);
  assert.match(workflow, /cargo fmt --all -- --check/);
  assert.match(workflow, /cargo clippy --locked --all-targets -- -D warnings/);
  assert.match(workflow, /cargo test --locked --all-targets/);
  assert.match(workflow, /gha-clone-server-config\.test\.ts/);
  assert.match(workflow, /gha-clone-server-meta\.yml/);
  assert.match(workflow, /msgint-operator-config\.yml/);
  assert.match(workflow, /dd-build-server-gha-continuity\.patch\.yaml/);
  assert.match(workflow, /actionlint@sha256:/);
  assert.match(workflow, /persist-credentials:\s*false/);
});
