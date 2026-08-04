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
const routerConfigPath =
  'remote/argocd/dd-next-runtime/dd-gha-executor-router.configmap.yaml';
const routerSecretPath =
  'remote/argocd/dd-next-runtime/dd-gha-executor-router.externalsecret.yaml';
const routerDeploymentPath =
  'remote/argocd/dd-next-runtime/dd-gha-executor-router.deployment.yaml';
const routerServicePath =
  'remote/argocd/dd-next-runtime/dd-gha-executor-router.service.yaml';
const routerNetworkPath =
  'remote/argocd/dd-next-runtime/dd-gha-executor-router.networkpolicy.yaml';
const kustomizationPath = 'remote/argocd/dd-next-runtime/kustomization.yaml';
const observabilityPaths = [
  'remote/argocd/observability/k8s-resource-exporter.configmap.yaml',
  'remote/argocd/observability/k8s-resource-exporter.deployment.yaml',
];
const profilesPath = 'remote/deployments/build-server-rs/src/profiles.rs';
const plannerPath = 'remote/deployments/gha-clone-server-rs/src/lib.rs';
const serverPath = 'remote/deployments/gha-clone-server-rs/src/main.rs';
const routerSourcePaths = [
  'remote/deployments/gha-clone-server-rs/src/bin/gha-executor-router.rs',
  'remote/deployments/gha-clone-server-rs/src/executor_router_service.rs',
  'remote/deployments/gha-clone-server-rs/src/executor_router_service/assignment.rs',
  'remote/deployments/gha-clone-server-rs/src/executor_router_service/security.rs',
  'remote/deployments/gha-clone-server-rs/src/executor_router_service/upstream.rs',
];
const routerLibraryPath =
  'remote/deployments/gha-clone-server-rs/src/executor_router.rs';
const routerTestPaths = [
  'remote/deployments/gha-clone-server-rs/tests/executor_router_http.rs',
  'remote/deployments/gha-clone-server-rs/tests/executor_router_assignment.rs',
];
const metaIntegrationTestPath =
  'remote/deployments/gha-clone-server-rs/tests/meta_self_test.rs';
const workflowPath = '.github/workflows/gha-clone-server.yml';
const metaWorkflowPath = '.github/workflows/gha-clone-server-meta.yml';

function assertInertDeployment(
  name: string,
  deployment: string,
  image: RegExp,
  command: RegExp,
  executionGate: RegExp,
): void {
  assert.match(deployment, /\breplicas:\s*0\b/, `${name} must stay scaled to zero`);
  assert.match(
    deployment,
    /\bautomountServiceAccountToken:\s*false\b/,
    `${name} must not receive cluster identity`,
  );
  assert.match(deployment, executionGate, `${name} execution must remain disabled`);
  assert.match(deployment, image, `${name} must retain the non-runnable digest sentinel`);
  assert.match(deployment, command, `${name} must use a compiled entrypoint`);
  assert.match(deployment, /readOnlyRootFilesystem:\s*true/);
  assert.match(deployment, /capabilities:\s+drop:\s*\["ALL"\]/);
  assert.doesNotMatch(deployment, /hostPath:/);
  assert.doesNotMatch(
    deployment,
    /docker\.sock|containerd\.sock|buildkitd\.sock|git[\s\S]{0,80}clone|cargo run|\/bin\/(?:ba)?sh/,
  );
}

test('clone server and executor router are immutable and inert by merge', () => {
  const clone = read(deploymentPath);
  const router = read(routerDeploymentPath);

  assertInertDeployment(
    'clone server',
    clone,
    /image:\s*ghcr\.io\/oresoftware\/gha-clone-server@sha256:0{64}/,
    /command:\s*\["\/usr\/local\/bin\/gha-clone-server"\]/,
    /name:\s*GHA_CLONE_EXECUTION_ENABLED\s+value:\s*"false"/,
  );
  assert.match(
    clone,
    /name:\s*GHA_CLONE_WEBHOOK_EXECUTION_ENABLED\s+value:\s*"false"/,
  );
  assert.match(clone, /name:\s*dd-gha-clone-server-secrets/);

  assertInertDeployment(
    'executor router',
    router,
    /image:\s*ghcr\.io\/oresoftware\/gha-executor-router@sha256:0{64}/,
    /command:\s*\["\/usr\/local\/bin\/gha-executor-router"\]/,
    /name:\s*GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED\s+value:\s*"false"/,
  );
  assert.doesNotMatch(router, /name:\s*GHA_EXECUTOR_ROUTER_SECRET_ROOT/);
  assert.match(
    router,
    /name:\s*GHA_EXECUTOR_ROUTER_AUTH_PATH\s+value:\s*\/var\/run\/secrets\/gha-executor-router\/inbound-auth/,
  );
  assert.match(
    router,
    /projected:\s+defaultMode:\s*0400[\s\S]*?name:\s*dd-gha-executor-router-secrets[\s\S]*?key:\s*inbound_auth[\s\S]*?path:\s*inbound-auth/,
  );
  assert.match(
    router,
    /name:\s*dd-agent-secrets[\s\S]*?key:\s*SERVER_AUTH_SECRET[\s\S]*?path:\s*aws-build-server-auth/,
  );
  assert.doesNotMatch(router, /hetzner-build-server-auth/);
});

test('all continuity resources render and remain observable', () => {
  const kustomization = read(kustomizationPath);
  for (const resource of [
    'dd-gha-clone-server.configmap.yaml',
    'dd-gha-clone-server.externalsecret.yaml',
    'dd-gha-clone-server.deployment.yaml',
    'dd-gha-clone-server.service.yaml',
    'dd-gha-clone-server.networkpolicy.yaml',
    'dd-gha-executor-router.configmap.yaml',
    'dd-gha-executor-router.externalsecret.yaml',
    'dd-gha-executor-router.deployment.yaml',
    'dd-gha-executor-router.service.yaml',
    'dd-gha-executor-router.networkpolicy.yaml',
  ]) {
    assert.ok(kustomization.includes(`- ${resource}`), `${resource} is not rendered`);
  }
  assert.match(read(servicePath), /port:\s*8125/);
  assert.match(read(routerServicePath), /port:\s*8126/);

  for (const path of observabilityPaths) {
    const inventory = read(path);
    assert.match(inventory, /dd-build-server,dd-gha-clone-server,/);
    assert.match(inventory, /dd-gha-clone-server,dd-gha-executor-router,/);
  }
});

test('secret ownership is non-duplicating and credential material is never committed', () => {
  const cloneSecret = read(secretPath);
  for (const property of [
    'auth_secret',
    'github_webhook_secret',
    'github_app_installation_token',
  ]) {
    assert.match(cloneSecret, new RegExp(`property: ${property}`));
  }
  assert.doesNotMatch(cloneSecret, /build_server_auth/);

  const routerSecret = read(routerSecretPath);
  const routerDeployment = read(routerDeploymentPath);
  assert.match(routerSecret, /property:\s*inbound_auth/);
  assert.doesNotMatch(routerSecret, /property:\s*aws_build_server_auth/);
  assert.doesNotMatch(routerSecret, /property:\s*hetzner_build_server_auth/);
  assert.match(routerSecret, /Do not duplicate it in this backing secret/);
  assert.match(
    routerDeployment,
    /name:\s*dd-agent-secrets[\s\S]*?key:\s*SERVER_AUTH_SECRET[\s\S]*?path:\s*aws-build-server-auth/,
  );

  for (const text of [cloneSecret, routerSecret, routerDeployment]) {
    assert.doesNotMatch(
      text,
      /ghp_[A-Za-z0-9]+|github_pat_[A-Za-z0-9_]+|BEGIN (?:RSA |EC )?PRIVATE KEY/,
    );
  }
});

test('AWS is the only enabled independent executor and Hetzner has no dormant authority', () => {
  const config = read(routerConfigPath);
  assert.match(
    config,
    /"id": "aws-primary"[\s\S]*?"provider": "aws"[\s\S]*?"enabled": true[\s\S]*?"url": "http:\/\/dd-build-server\.default\.svc\.cluster\.local:8100"[\s\S]*?"authPath": "\/var\/run\/secrets\/gha-executor-router\/aws-build-server-auth"/,
  );
  assert.match(
    config,
    /"id": "hetzner-secondary",\s*"provider": "hetzner",\s*"enabled": false\s*\}/,
  );
  const hetzner = config.slice(config.indexOf('"id": "hetzner-secondary"'));
  assert.doesNotMatch(hetzner, /"url"|"authPath"/);

  const policy = read(routerNetworkPath);
  assert.match(policy, /app:\s*dd-gha-clone-server/);
  assert.match(policy, /port:\s*8126/);
  assert.match(policy, /port:\s*53/);
  assert.match(policy, /app:\s*dd-build-server/);
  assert.match(policy, /port:\s*8100/);
  assert.match(policy, /No public egress exists while Hetzner is disabled/);
  assert.doesNotMatch(policy, /port:\s*443|ipBlock:|cidr:/);
});

test('clone dispatch is forced through the authenticated router', () => {
  const deployment = read(deploymentPath);
  const network = read(networkPath);
  assert.match(
    deployment,
    /name:\s*GHA_CLONE_BUILD_SERVER_URL\s+value:\s*http:\/\/dd-gha-executor-router\.default\.svc\.cluster\.local:8126/,
  );
  assert.match(
    deployment,
    /name:\s*GHA_CLONE_BUILD_SERVER_AUTH[\s\S]*?name:\s*dd-gha-executor-router-secrets[\s\S]*?key:\s*inbound_auth/,
  );
  assert.match(network, /app:\s*dd-gha-executor-router/);
  assert.match(network, /port:\s*8126/);
  assert.doesNotMatch(network, /app:\s*dd-build-server|port:\s*8100/);
});

test('configuration and fixed profiles preserve the bounded execution surface', () => {
  const config = read(configMapPath);
  assert.match(config, /ORESoftware\/k8s-cluster/);
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
  assert.doesNotMatch(config, /https?:\/\/[^/\s]+\/\*|owner\/\*/);

  const profiles = read(profilesPath);
  for (const profile of ['rust-verify', 'node-verify', 'python-verify']) {
    assert.match(profiles, new RegExp(`name: "${profile}"`));
  }
  assert.match(
    profiles,
    /cargo clippy --locked --all-targets --all-features -- -D warnings/,
  );
  assert.match(profiles, /pnpm install --frozen-lockfile/);
  assert.match(profiles, /python -m pytest/);
  assert.doesNotMatch(profiles, /find .*Cargo\.toml|for crate in/);

  for (const [, image] of profiles.matchAll(
    /const\s+[A-Z_]+_IMAGE:\s*&str\s*=\s*"([^"]+)";/g,
  )) {
    assert.ok(!image.endsWith(':latest'), `runner image must be pinned: ${image}`);
  }
});

test('planner and dispatcher reject arbitrary workflow execution', () => {
  const planner = read(plannerPath);
  const server = read(serverPath);
  assert.match(planner, /service containers require the isolated ARC DinD lane/);
  assert.match(planner, /secret-bearing env\/with values are unsupported/);
  assert.match(planner, /workflow job dependency graph contains a cycle/);
  assert.match(planner, /revision is not an exact 40-hex commit SHA/);
  assert.match(planner, /workflow-level .* is unsupported by the independent lane/);
  assert.match(planner, /"working-directory"/);
  assert.match(planner, /non-Linux native execution is unavailable/);
  assert.match(server, /job_kind: "run-profile"/);
  assert.match(server, /profile,/);
  assert.doesNotMatch(server, /command:\s*&|script:\s*&|runner_image/);
});

test('current router modules preserve request assignment and provider pinning', () => {
  const source = routerSourcePaths.map(read).join('\n');
  const library = read(routerLibraryPath);
  const processTests = routerTestPaths.map(read).join('\n');

  for (const expression of [
    /postSubmissionFailover": false/,
    /ambiguous_submissions_total/,
    /first_ready_executor/,
    /automaticFailover": false/,
    /get_all\("x-build-server-auth"\)/,
    /get_all\("x-server-auth"\)/,
  ]) {
    assert.match(source, expression);
  }
  assert.match(library, /disabled executors must omit url and authPath/);
  assert.match(library, /authPath must be a direct child/);
  assert.match(library, /lowercase 40-hex commit SHA/);

  for (const expression of [
    /selects_first_ready_aws_executor_and_pins_status_to_it/,
    /readiness_failure_routes_to_hetzner_before_any_submission/,
    /ambiguous_submission_never_fails_over_or_leaks_upstream_body/,
    /accepted_build_status_failure_remains_pinned_without_resubmission/,
    /sequential_and_concurrent_identical_requests_submit_once/,
    /ambiguous_assignment_is_retained_and_retry_never_switches_provider/,
  ]) {
    assert.match(processTests, expression);
  }
});

test('meta workflow remains independently compilable and testable', () => {
  const integration = read(metaIntegrationTestPath);
  assert.match(integration, /CARGO_BIN_EXE_gha-clone-server/);
  assert.match(integration, /\/v1\/runs/);
  assert.match(integration, /MockBuildState/);
  assert.match(integration, /gha-clone-server-meta\.yml/);
  assert.match(integration, /"profile"\], "rust-verify"/);
  assert.match(integration, /GHA_CLONE_EXECUTION_ENABLED", "true"/);
  assert.match(integration, /env_remove\("GHA_CLONE_GITHUB_TOKEN"\)/);

  const workflow = read(metaWorkflowPath);
  assert.match(workflow, /workflow_dispatch:/);
  assert.match(workflow, /gha-clone-self-test:/);
  assert.match(
    workflow,
    /cargo test --manifest-path remote\/deployments\/gha-clone-server-rs\/Cargo\.toml --locked --all-targets/,
  );
  assert.match(workflow, /persist-credentials:\s*false/);
  assert.doesNotMatch(workflow, /\$\{\{\s*secrets\./);
  assert.doesNotMatch(workflow, /services:|container:|strategy:|needs:/);
});

test('permanent GHA workflow checks source, deployment, activation, and overlay contracts', () => {
  const workflow = read(workflowPath);
  assert.match(workflow, /cargo fmt --all -- --check/);
  assert.match(workflow, /cargo clippy --locked --all-targets -- -D warnings/);
  assert.match(workflow, /cargo test --locked --all-targets/);
  assert.match(workflow, /cargo test --locked --test executor_router_http/);
  assert.match(workflow, /gha-clone-server-config\.test\.ts/);
  assert.match(workflow, /gha-executor-router-activation\.test\.mjs/);
  assert.match(workflow, /kubectl kustomize remote\/argocd\/dd-next-runtime/);
  assert.match(workflow, /actionlint@sha256:/);
  assert.match(workflow, /persist-credentials:\s*false/);
});
