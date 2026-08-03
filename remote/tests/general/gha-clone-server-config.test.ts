import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const root = join(import.meta.dirname, '../../..');
const read = (path: string) => readFileSync(join(root, path), 'utf8');

const deploymentPath =
  'argocd/dd-next-runtime/dd-gha-clone-server.deployment.yaml';
const configMapPath =
  'argocd/dd-next-runtime/dd-gha-clone-server.configmap.yaml';
const secretPath =
  'argocd/dd-next-runtime/dd-gha-clone-server.externalsecret.yaml';
const networkPath =
  'argocd/dd-next-runtime/dd-gha-clone-server.networkpolicy.yaml';
const servicePath =
  'argocd/dd-next-runtime/dd-gha-clone-server.service.yaml';
const kustomizationPath = 'argocd/dd-next-runtime/kustomization.yaml';
const profilesPath = 'deployments/build-server-rs/src/profiles.rs';
const plannerPath = 'deployments/gha-clone-server-rs/src/lib.rs';
const serverPath = 'deployments/gha-clone-server-rs/src/main.rs';
const workflowPath = '../.github/workflows/gha-clone-server.yml';

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
    assert.match(kustomization, new RegExp(`- ${resource.replaceAll('.', '\\.')}`));
  }
  assert.match(read(servicePath), /port:\s*8125/);
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

test('config allowlists exact trusted repositories and static workflow paths', () => {
  const config = read(configMapPath);
  assert.match(config, /ORESoftware\/k8s-cluster/);
  assert.match(config, /sonus-auris\/sonus-auris-interfaces/);
  assert.match(config, /\.github\/workflows\/ci\.yml/);
  assert.doesNotMatch(config, /https?:\/\/[^/\s]+\/\*|owner\/\*/);
});

test('build server exposes fixed Rust, Node, and Python continuity profiles', () => {
  const profiles = read(profilesPath);
  for (const profile of ['rust-verify', 'node-verify', 'python-verify']) {
    assert.match(profiles, new RegExp(`name: "${profile}"`));
  }
  assert.match(profiles, /cargo clippy --all-targets --all-features -- -D warnings/);
  assert.match(profiles, /pnpm install --frozen-lockfile/);
  assert.match(profiles, /python -m pytest/);
  assert.doesNotMatch(profiles, /:latest"/);
});

test('planner and dispatcher preserve the fail-closed command boundary', () => {
  const planner = read(plannerPath);
  const server = read(serverPath);
  assert.match(planner, /service containers require the isolated ARC DinD lane/);
  assert.match(planner, /secret-bearing env\/with values are unsupported/);
  assert.match(planner, /workflow job dependency graph contains a cycle/);
  assert.match(planner, /revision is not an exact 40-hex commit SHA/);
  assert.match(server, /job_kind: "run-profile"/);
  assert.match(server, /profile,/);
  assert.doesNotMatch(server, /command:\s*&|script:\s*&|runner_image/);
});

test('dedicated GitHub Actions workflow checks Rust and deployment contracts', () => {
  const workflow = read(workflowPath);
  assert.match(workflow, /cargo fmt --all -- --check/);
  assert.match(workflow, /cargo clippy --locked --all-targets -- -D warnings/);
  assert.match(workflow, /cargo test --locked --all-targets/);
  assert.match(workflow, /gha-clone-server-config\.test\.ts/);
  assert.match(workflow, /actionlint@sha256:/);
  assert.match(workflow, /persist-credentials:\s*false/);
});
