import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const root = join(import.meta.dirname, '../../..');
const read = (path: string) => readFileSync(join(root, path), 'utf8');

const deploymentPath =
  'remote/argocd/dd-next-runtime/dd-gha-executor-router.deployment.yaml';
const configPath =
  'remote/argocd/dd-next-runtime/dd-gha-executor-router.configmap.yaml';
const secretPath =
  'remote/argocd/dd-next-runtime/dd-gha-executor-router.externalsecret.yaml';
const servicePath =
  'remote/argocd/dd-next-runtime/dd-gha-executor-router.service.yaml';
const networkPath =
  'remote/argocd/dd-next-runtime/dd-gha-executor-router.networkpolicy.yaml';
const kustomizationPath = 'remote/argocd/dd-next-runtime/kustomization.yaml';
const sourcePath = 'remote/deployments/gha-executor-router-rs/src/lib.rs';
const workflowPath = '.github/workflows/gha-executor-router.yml';

test('executor router is installed inert with no cluster or host identity', () => {
  const deployment = read(deploymentPath);
  assert.match(deployment, /\breplicas:\s*0\b/);
  assert.match(deployment, /\bautomountServiceAccountToken:\s*false\b/);
  assert.match(
    deployment,
    /name:\s*GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED\s+value:\s*"false"/,
  );
  assert.match(deployment, /capabilities:\s+drop:\s*\["ALL"\]/);
  assert.doesNotMatch(deployment, /hostPath:/);
  assert.doesNotMatch(
    deployment,
    /docker\.sock|containerd\.sock|buildkitd\.sock|AWS_ACCESS_KEY|HCLOUD_TOKEN/,
  );
  assert.doesNotMatch(
    deployment,
    /name:\s*(?:GHA_EXECUTOR_ROUTER_AUTH|AWS_BUILD_SERVER_AUTH|HETZNER_BUILD_SERVER_AUTH)\s+valueFrom:/,
  );
});

test('operator and provider credentials are distinct mounted files', () => {
  const deployment = read(deploymentPath);
  for (const [key, path] of [
    ['operator_auth', 'operator-auth'],
    ['aws_build_server_auth', 'aws-auth'],
    ['hetzner_build_server_auth', 'hetzner-auth'],
  ]) {
    assert.match(deployment, new RegExp(`key: ${key}\\s+path: ${path}`));
  }
  assert.match(
    deployment,
    /GHA_EXECUTOR_ROUTER_AUTH_SECRET_FILE\s+value:\s*\/var\/run\/secrets\/gha-executor-router\/operator-auth/,
  );
  assert.match(deployment, /mountPath:\s*\/var\/run\/secrets\/gha-executor-router/);
  assert.match(deployment, /readOnly:\s*true/);
});

test('ordered authorities are exact, unique, and activation-gated', () => {
  const config = read(configPath);
  assert.match(config, /"id":"aws","provider":"aws"/);
  assert.match(config, /dd-build-server\.default\.svc\.cluster\.local:8100/);
  assert.match(config, /"id":"hetzner","provider":"hetzner"/);
  assert.match(config, /replace-before-activation\.invalid/);
  assert.match(config, /\/gha-executor-router\/aws-auth/);
  assert.match(config, /\/gha-executor-router\/hetzner-auth/);
  assert.match(config, /GHA_EXECUTOR_ROUTER_MAX_EXECUTORS:\s*"2"/);
  assert.doesNotMatch(config, /https?:\/\/[^"\s]+[?#]/);
});

test('ExternalSecret names fields without committing credential material', () => {
  const secret = read(secretPath);
  assert.match(secret, /dd\/remote-dev\/gha-executor-router-secrets/);
  for (const property of [
    'operator_auth',
    'aws_build_server_auth',
    'hetzner_build_server_auth',
  ]) {
    assert.match(secret, new RegExp(`property: ${property}`));
  }
  assert.doesNotMatch(
    secret,
    /ghp_[A-Za-z0-9]+|github_pat_[A-Za-z0-9_]+|BEGIN (?:RSA |EC )?PRIVATE KEY/,
  );
});

test('network policy admits only mirror ingress and bounded executor egress', () => {
  const policy = read(networkPath);
  assert.match(policy, /app:\s*dd-gha-clone-server/);
  assert.match(policy, /port:\s*8126/);
  assert.match(policy, /port:\s*53/);
  assert.match(policy, /app:\s*dd-build-server/);
  assert.match(policy, /port:\s*8100/);
  assert.match(policy, /port:\s*443/);
  for (const blocked of [
    '10\\.0\\.0\\.0/8',
    '169\\.254\\.0\\.0/16',
    '172\\.16\\.0\\.0/12',
    '192\\.168\\.0\\.0/16',
  ]) {
    assert.match(policy, new RegExp(blocked));
  }
});

test('router resources participate in dd-next-runtime rendering', () => {
  const kustomization = read(kustomizationPath);
  for (const resource of [
    'dd-gha-executor-router.configmap.yaml',
    'dd-gha-executor-router.externalsecret.yaml',
    'dd-gha-executor-router.deployment.yaml',
    'dd-gha-executor-router.service.yaml',
    'dd-gha-executor-router.networkpolicy.yaml',
  ]) {
    assert.ok(
      kustomization.includes(`- ${resource}`),
      `${resource} is missing from dd-next-runtime kustomization`,
    );
  }
  assert.match(read(servicePath), /port:\s*8126/);
});

test('Rust boundary fails over only before acceptance and rejects arbitrary execution', () => {
  const source = read(sourcePath);
  assert.match(source, /status == StatusCode::TOO_MANY_REQUESTS \|\| status\.is_server_error\(\)/);
  assert.match(source, /fallback was not attempted/);
  assert.match(source, /the job was not resubmitted/);
  assert.match(source, /duplicate_request_id_is_submitted_once_even_when_concurrent/);
  assert.match(source, /accepted_aws_poll_failure_stays_pinned_and_never_resubmits/);
  assert.match(source, /deny_unknown_fields/);
  assert.match(source, /only the operator-reviewed run-profile job kind is routable/);
  assert.doesNotMatch(source, /Command::new|std::process::Command|docker\.sock|hostPath:/);
});

test('dedicated workflow checks Rust, static policy, and optional ARC parity', () => {
  const workflow = read(workflowPath);
  assert.match(workflow, /cargo fmt --all --check/);
  assert.match(workflow, /cargo test --manifest-path/);
  assert.match(workflow, /cargo clippy --manifest-path/);
  assert.match(workflow, /-D warnings/);
  assert.match(workflow, /persist-credentials:\s*false/);
  assert.match(workflow, /runs-on:\s*\[self-hosted, linux, sonus-ci\]/);
  assert.match(workflow, /run_arc_smoke/);
  assert.doesNotMatch(workflow, /\$\{\{\s*secrets\./);
});
