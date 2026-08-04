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
const observabilityConfigPath =
  'remote/argocd/observability/k8s-resource-exporter.configmap.yaml';
const observabilityDeploymentPath =
  'remote/argocd/observability/k8s-resource-exporter.deployment.yaml';
const profilesPath = 'remote/deployments/build-server-rs/src/profiles.rs';
const plannerPath = 'remote/deployments/gha-clone-server-rs/src/lib.rs';
const serverPath = 'remote/deployments/gha-clone-server-rs/src/main.rs';
const routerSourcePath =
  'remote/deployments/gha-clone-server-rs/src/bin/gha-executor-router.rs';
const routerServiceImplementationPath =
  'remote/deployments/gha-clone-server-rs/src/executor_router_service.rs';
const routerAssignmentImplementationPath =
  'remote/deployments/gha-clone-server-rs/src/executor_router_service/assignment.rs';
const routerSecurityImplementationPath =
  'remote/deployments/gha-clone-server-rs/src/executor_router_service/security.rs';
const routerUpstreamImplementationPath =
  'remote/deployments/gha-clone-server-rs/src/executor_router_service/upstream.rs';
const routerLibraryPath =
  'remote/deployments/gha-clone-server-rs/src/executor_router.rs';
const routerProcessTestPath =
  'remote/deployments/gha-clone-server-rs/tests/executor_router_http.rs';
const routerAssignmentTestPath =
  'remote/deployments/gha-clone-server-rs/tests/executor_router_assignment.rs';
const metaIntegrationTestPath =
  'remote/deployments/gha-clone-server-rs/tests/meta_self_test.rs';
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

test('source-bootstrap pods cannot be activated before immutable images exist', () => {
  const cloneDeployment = read(deploymentPath);
  const routerDeployment = read(routerDeploymentPath);
  for (const [name, deployment, executionGate] of [
    [
      'clone server',
      cloneDeployment,
      /name:\s*GHA_CLONE_EXECUTION_ENABLED\s+value:\s*"false"/,
    ],
    [
      'executor router',
      routerDeployment,
      /name:\s*GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED\s+value:\s*"false"/,
    ],
  ] as const) {
    assert.match(deployment, /\breplicas:\s*0\b/, `${name} must stay scaled to zero`);
    assert.match(deployment, executionGate, `${name} execution must stay disabled`);
    assert.match(deployment, /image:\s*docker\.io\/library\/rust:1\.90-bookworm/);
    assert.match(deployment, /git[\s\S]*?clone[\s\S]*?cargo run --release/);
    assert.doesNotMatch(
      deployment,
      /image:\s*\S+@sha256:[0-9a-f]{64}/,
      `${name} bootstrap manifest must be replaced atomically by an immutable-image activation change`,
    );
  }
});

test('all continuity and executor-router resources participate in the shared render', () => {
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
    assert.ok(
      kustomization.includes(`- ${resource}`),
      `${resource} is missing from dd-next-runtime kustomization`,
    );
  }
  assert.match(read(servicePath), /port:\s*8125/);
  assert.match(read(routerServicePath), /port:\s*8126/);
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

test('clone-server secret mapping names values without committing credential material', () => {
  const secret = read(secretPath);
  assert.match(secret, /dd\/remote-dev\/gha-clone-server-secrets/);
  for (const property of [
    'auth_secret',
    'github_webhook_secret',
    'github_app_installation_token',
  ]) {
    assert.match(secret, new RegExp(`property: ${property}`));
  }
  assert.doesNotMatch(secret, /build_server_auth/);
  assert.match(secret, /Direct executor credentials belong only to/);
  assert.doesNotMatch(
    secret,
    /ghp_[A-Za-z0-9]+|github_pat_[A-Za-z0-9_]+|BEGIN (?:RSA |EC )?PRIVATE KEY/,
  );
});

test('executor router is rendered inert and has no cluster or host-runtime identity', () => {
  const deployment = read(routerDeploymentPath);
  assert.match(deployment, /\breplicas:\s*0\b/);
  assert.match(deployment, /\bautomountServiceAccountToken:\s*false\b/);
  assert.match(
    deployment,
    /name:\s*GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED\s+value:\s*"false"/,
  );
  assert.match(deployment, /cargo run --release --bin gha-executor-router/);
  assert.match(
    deployment,
    /name:\s*GHA_EXECUTOR_ROUTER_SECRET_ROOT\s+value:\s*\/var\/run\/secrets\/gha-executor-router/,
  );
  assert.match(
    deployment,
    /name:\s*GHA_EXECUTOR_ROUTER_AUTH_PATH\s+value:\s*\/var\/run\/secrets\/gha-executor-router\/inbound-auth/,
  );
  assert.match(deployment, /secretName:\s*dd-gha-executor-router-secrets/);
  assert.match(deployment, /defaultMode:\s*0400/);
  assert.match(deployment, /key:\s*inbound_auth\s+path:\s*inbound-auth/);
  assert.match(
    deployment,
    /key:\s*aws_build_server_auth\s+path:\s*aws-build-server-auth/,
  );
  assert.match(deployment, /capabilities:\s+drop:\s*\["ALL"\]/);
  assert.doesNotMatch(deployment, /hostPath:/);
  assert.doesNotMatch(deployment, /docker\.sock|containerd\.sock|buildkitd\.sock/);
  assert.doesNotMatch(deployment, /valueFrom:\s*\n\s*secretKeyRef:[\s\S]{0,240}GHA_EXECUTOR_ROUTER/);
});

test('executor inventory enables AWS only and keeps Hetzner endpoint state absent', () => {
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
});

test('executor credentials are separate mounted-file properties and omit dormant Hetzner auth', () => {
  const secret = read(routerSecretPath);
  assert.match(secret, /dd\/remote-dev\/gha-executor-router-secrets/);
  assert.match(secret, /property:\s*inbound_auth/);
  assert.match(secret, /property:\s*aws_build_server_auth/);
  assert.doesNotMatch(secret, /property:\s*hetzner_build_server_auth/);
  assert.match(secret, /Do not add or mount hetzner_build_server_auth/);
  assert.doesNotMatch(
    secret,
    /ghp_[A-Za-z0-9]+|github_pat_[A-Za-z0-9_]+|BEGIN (?:RSA |EC )?PRIVATE KEY/,
  );
});

test('clone server dispatch is forced through the authenticated router', () => {
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
  assert.doesNotMatch(network, /app:\s*dd-build-server/);
  assert.doesNotMatch(network, /port:\s*8100/);
});

test('router network boundary permits clone ingress, DNS, AWS dispatch, and reviewed HTTPS only', () => {
  const policy = read(routerNetworkPath);
  assert.match(policy, /app:\s*dd-gha-clone-server/);
  assert.match(policy, /port:\s*8126/);
  assert.match(policy, /port:\s*53/);
  assert.match(policy, /app:\s*dd-build-server/);
  assert.match(policy, /port:\s*8100/);
  assert.match(policy, /port:\s*443/);
  assert.match(policy, /10\.0\.0\.0\/8/);
  assert.match(policy, /100\.64\.0\.0\/10/);
  assert.match(policy, /169\.254\.0\.0\/16/);
  assert.match(policy, /192\.168\.0\.0\/16/);
});

test('config allowlists exact trusted repositories and the bounded meta workflow', () => {
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
});

test('build server exposes fixed Rust, Node, and Python continuity profiles', () => {
  const profiles = read(profilesPath);
  for (const profile of ['rust-verify', 'node-verify', 'python-verify']) {
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
  assert.match(profiles, /python -m pytest/);
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
  assert.match(server, /job_kind: "run-profile"/);
  assert.match(server, /profile,/);
  assert.doesNotMatch(server, /command:\s*&|script:\s*&|runner_image/);
});

test('executor router code and live tests preserve no-duplicate provider pinning', () => {
  const source = [
    routerSourcePath,
    routerServiceImplementationPath,
    routerAssignmentImplementationPath,
    routerSecurityImplementationPath,
    routerUpstreamImplementationPath,
  ]
    .map(read)
    .join('\n');
  const library = read(routerLibraryPath);
  const processTests = [routerProcessTestPath, routerAssignmentTestPath]
    .map(read)
    .join('\n');
  assert.match(source, /postSubmissionFailover": false/);
  assert.match(source, /ambiguous_submissions_total/);
  assert.match(source, /first_ready_executor/);
  assert.match(source, /automaticFailover": false/);
  assert.match(source, /get_all\("x-build-server-auth"\)/);
  assert.match(source, /get_all\("x-server-auth"\)/);
  assert.match(library, /disabled executors must omit url and authPath/);
  assert.match(library, /authPath must be a direct child/);
  assert.match(library, /lowercase 40-hex commit SHA/);
  assert.match(
    processTests,
    /selects_first_ready_aws_executor_and_pins_status_to_it/,
  );
  assert.match(
    processTests,
    /readiness_failure_routes_to_hetzner_before_any_submission/,
  );
  assert.match(
    processTests,
    /ambiguous_submission_never_fails_over_or_leaks_upstream_body/,
  );
  assert.match(
    processTests,
    /accepted_build_status_failure_remains_pinned_without_resubmission/,
  );
  assert.match(
    processTests,
    /sequential_and_concurrent_identical_requests_submit_once/,
  );
  assert.match(
    processTests,
    /ambiguous_assignment_is_retained_and_retry_never_switches_provider/,
  );
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

test('dedicated GitHub Actions workflow checks Rust, live router, and deployment contracts', () => {
  const workflow = read(workflowPath);
  assert.match(workflow, /cargo fmt --all -- --check/);
  assert.match(workflow, /cargo clippy --locked --all-targets -- -D warnings/);
  assert.match(workflow, /cargo test --locked --all-targets/);
  assert.match(workflow, /cargo test --locked --test executor_router_http/);
  assert.match(workflow, /dd-gha-executor-router\*/);
  assert.match(workflow, /gha-clone-server-config\.test\.ts/);
  assert.match(workflow, /gha-clone-server-meta\.yml/);
  assert.match(workflow, /actionlint@sha256:/);
  assert.match(workflow, /persist-credentials:\s*false/);
});
