import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '../../..');
const read = (path) => readFileSync(resolve(root, path), 'utf8');

const paths = {
  configMap: 'remote/argocd/dd-next-runtime/dd-gha-executor-router.configmap.yaml',
  externalSecret:
    'remote/argocd/dd-next-runtime/dd-gha-executor-router.externalsecret.yaml',
  deployment:
    'remote/argocd/dd-next-runtime/dd-gha-executor-router.deployment.yaml',
  service: 'remote/argocd/dd-next-runtime/dd-gha-executor-router.service.yaml',
  networkPolicy:
    'remote/argocd/dd-next-runtime/dd-gha-executor-router.networkpolicy.yaml',
  cloneDeployment:
    'remote/argocd/dd-next-runtime/dd-gha-clone-server.deployment.yaml',
  cloneExternalSecret:
    'remote/argocd/dd-next-runtime/dd-gha-clone-server.externalsecret.yaml',
  cloneNetworkPolicy:
    'remote/argocd/dd-next-runtime/dd-gha-clone-server.networkpolicy.yaml',
  kustomization: 'remote/argocd/dd-next-runtime/kustomization.yaml',
  workflow: '.github/workflows/gha-clone-server.yml',
  routerSource:
    'remote/deployments/gha-clone-server-rs/src/bin/gha-executor-router.rs',
  routerContracts:
    'remote/deployments/gha-clone-server-rs/src/executor_router.rs',
  routerTests:
    'remote/deployments/gha-clone-server-rs/tests/executor_router_http.rs',
};

function extractLiteralBlock(text, key) {
  const marker = `  ${key}: |\n`;
  const start = text.indexOf(marker);
  assert.notEqual(start, -1, `missing literal block ${key}`);
  const lines = text.slice(start + marker.length).split('\n');
  const block = [];
  for (const line of lines) {
    if (line.length === 0) {
      block.push('');
      continue;
    }
    if (!line.startsWith('    ')) break;
    block.push(line.slice(4));
  }
  return block.join('\n').trim();
}

function requireContains(text, values, label) {
  for (const value of values) {
    assert.ok(text.includes(value), `${label} missing ${value}`);
  }
}

function envValue(text, name) {
  const pattern = new RegExp(
    `- name: ${name}\\n\\s+value: (?:(?:"([^"\\n]*)")|([^\\n#]+))`,
  );
  const match = text.match(pattern);
  assert.ok(match, `missing literal environment value ${name}`);
  return (match[1] ?? match[2]).trim();
}

test('executor inventory is exact and carries no dormant Hetzner endpoint', () => {
  const configMap = read(paths.configMap);
  const executors = JSON.parse(
    extractLiteralBlock(configMap, 'GHA_EXECUTOR_ROUTER_EXECUTORS_JSON'),
  );
  assert.equal(executors.length, 2);
  assert.deepEqual(executors[0], {
    id: 'aws-primary',
    provider: 'aws',
    enabled: true,
    url: 'http://dd-build-server.default.svc.cluster.local:8100',
    authPath:
      '/var/run/secrets/gha-executor-router/aws-build-server-auth',
  });
  assert.deepEqual(executors[1], {
    id: 'hetzner-secondary',
    provider: 'hetzner',
    enabled: false,
  });
  assert.equal('url' in executors[1], false);
  assert.equal('authPath' in executors[1], false);
  assert.equal(new Set(executors.map(({ id }) => id)).size, executors.length);
});

test('router deployment is rendered but remains absent and execution-disabled', () => {
  const deployment = read(paths.deployment);
  requireContains(
    deployment,
    [
      'name: dd-gha-executor-router',
      'replicas: 0',
      'type: Recreate',
      'automountServiceAccountToken: false',
      'exec cargo run --release --bin gha-executor-router',
      'name: GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED',
      'value: "false"',
      'name: GHA_EXECUTOR_ROUTER_AUTH_PATH',
      'value: /var/run/secrets/gha-executor-router/router-auth',
      'mountPath: /var/run/secrets/gha-executor-router',
      'readOnly: true',
      'secretName: dd-gha-executor-router-secrets',
      'key: router_auth',
      'key: aws_build_server_auth',
      'path: /readyz',
      'path: /healthz',
      'drop: ["ALL"]',
      'readOnlyRootFilesystem: true',
    ],
    'router deployment',
  );
  assert.equal(envValue(deployment, 'GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED'), 'false');
  assert.doesNotMatch(deployment, /hostPath:|docker\.sock|containerd\.sock/);
  assert.doesNotMatch(deployment, /value:\s*(?:ghp_|github_pat_)/i);
});

test('ExternalSecret separates inbound router and AWS credentials and omits Hetzner', () => {
  const externalSecret = read(paths.externalSecret);
  requireContains(
    externalSecret,
    [
      'key: dd/remote-dev/gha-executor-router-secrets',
      'secretKey: router_auth',
      'property: router_auth',
      'secretKey: aws_build_server_auth',
      'property: aws_build_server_auth',
    ],
    'router ExternalSecret',
  );
  assert.equal(
    (externalSecret.match(/secretKey:/g) ?? []).length,
    2,
    'unexpected router secret authority added',
  );
  assert.doesNotMatch(externalSecret, /hetzner/i);
  assert.doesNotMatch(externalSecret, /ghp_|github_pat_|stringData:/i);
});

test('clone server dispatches only through the internal router', () => {
  const deployment = read(paths.cloneDeployment);
  requireContains(
    deployment,
    [
      'replicas: 0',
      'name: GHA_CLONE_EXECUTION_ENABLED',
      'name: GHA_CLONE_WEBHOOK_EXECUTION_ENABLED',
      'name: GHA_CLONE_BUILD_SERVER_URL',
      'value: http://dd-gha-executor-router.default.svc.cluster.local:8126',
      'name: GHA_CLONE_BUILD_SERVER_AUTH',
      'key: executor_router_auth',
    ],
    'clone-server deployment',
  );
  assert.equal(envValue(deployment, 'GHA_CLONE_EXECUTION_ENABLED'), 'false');
  assert.equal(
    envValue(deployment, 'GHA_CLONE_WEBHOOK_EXECUTION_ENABLED'),
    'false',
  );
  assert.ok(
    !deployment.includes(
      'value: http://dd-build-server.default.svc.cluster.local:8100',
    ),
    'clone server still bypasses the provider router',
  );

  const externalSecret = read(paths.cloneExternalSecret);
  requireContains(
    externalSecret,
    [
      'secretKey: executor_router_auth',
      'key: dd/remote-dev/gha-executor-router-secrets',
      'property: router_auth',
    ],
    'clone-server ExternalSecret',
  );
});

test('network policy permits only clone-to-router, router-to-AWS, and DNS', () => {
  const routerPolicy = read(paths.networkPolicy);
  requireContains(
    routerPolicy,
    [
      'app: dd-gha-executor-router',
      'app: dd-gha-clone-server',
      'port: 8126',
      'app: dd-build-server',
      'port: 8100',
      'kubernetes.io/metadata.name: kube-system',
      'port: 53',
      'No public HTTPS egress is admitted while the Hetzner executor is disabled.',
    ],
    'router NetworkPolicy',
  );
  assert.doesNotMatch(routerPolicy, /cidr:\s*0\.0\.0\.0\/0/);
  assert.doesNotMatch(routerPolicy, /port:\s*443/);

  const clonePolicy = read(paths.cloneNetworkPolicy);
  requireContains(
    clonePolicy,
    ['app: dd-gha-executor-router', 'port: 8126'],
    'clone-server NetworkPolicy',
  );
  const egress = clonePolicy.split('  egress:\n')[1] ?? '';
  assert.ok(!egress.includes('port: 8100'), 'clone server retains direct build egress');
});

test('disabled Hetzner state is consistent across inventory, credentials, and egress', () => {
  const configMap = read(paths.configMap);
  const externalSecret = read(paths.externalSecret);
  const networkPolicy = read(paths.networkPolicy);
  const deployment = read(paths.deployment);

  assert.match(configMap, /"id": "hetzner-secondary"/);
  assert.match(configMap, /"provider": "hetzner"/);
  assert.match(configMap, /"enabled": false/);
  assert.doesNotMatch(configMap, /hetzner[^\n]*(?:url|authPath)/i);
  assert.doesNotMatch(externalSecret, /hetzner/i);
  assert.doesNotMatch(networkPolicy, /port:\s*443|cidr:\s*0\.0\.0\.0\/0/);
  assert.equal(envValue(deployment, 'GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED'), 'false');
});

test('service and kustomization make the zero-replica router Argo-trackable', () => {
  const service = read(paths.service);
  requireContains(
    service,
    [
      'name: dd-gha-executor-router',
      'app: dd-gha-executor-router',
      'port: 8126',
      'targetPort: http',
    ],
    'router Service',
  );
  const kustomization = read(paths.kustomization);
  for (const filename of [
    'dd-gha-executor-router.configmap.yaml',
    'dd-gha-executor-router.externalsecret.yaml',
    'dd-gha-executor-router.deployment.yaml',
    'dd-gha-executor-router.service.yaml',
    'dd-gha-executor-router.networkpolicy.yaml',
  ]) {
    assert.ok(kustomization.includes(`  - ${filename}`), `kustomization missing ${filename}`);
  }
});

test('clean router core preserves readiness-only failover and ambiguous-POST pinning', () => {
  const source = read(paths.routerSource);
  const contracts = read(paths.routerContracts);
  const tests = read(paths.routerTests);

  requireContains(
    contracts,
    [
      'disabled executors must omit url and authPath',
      'authPath must be a direct child',
      'jobKind must be run-profile',
      'gitRef must be a lowercase 40-hex commit SHA',
      'plain HTTP is allowed only for loopback or in-cluster',
      'namespace_build_id',
      'parse_namespaced_build_id',
    ],
    'router contract module',
  );
  requireContains(
    source,
    [
      'first_ready_executor',
      'preSubmissionFailover',
      'postSubmissionFailover',
      'requestIdForwardedUnchanged',
      'crossProviderResubmissionRequires',
      'ambiguous_submissions_total',
      'StatusCode::TOO_MANY_REQUESTS',
      'redirect(reqwest::redirect::Policy::none())',
      'x-build-server-auth',
      'automaticFailover',
    ],
    'router server',
  );
  requireContains(
    tests,
    [
      'readiness_failure_routes_to_hetzner_before_any_submission',
      'ambiguous_submission_never_fails_over_or_leaks_upstream_body',
      'accepted_build_status_failure_remains_pinned_without_resubmission',
      'explicit_rejection_does_not_submit_to_the_second_provider',
    ],
    'router HTTP tests',
  );
  assert.doesNotMatch(source, /Command::new|\/bin\/bash|callerSelectedEndpoint\": true/);
});

test('continuity workflow exercises router contracts and renders the full overlay', () => {
  const workflow = read(paths.workflow);
  requireContains(
    workflow,
    [
      'general/gha-executor-router-config.test.mjs',
      'cargo clippy --locked --all-targets -- -D warnings',
      'cargo test --locked --all-targets',
      'kubectl kustomize remote/argocd/dd-next-runtime',
      'persist-credentials: false',
    ],
    'continuity workflow',
  );
});

test('changed router surface contains no committed credential markers', () => {
  const combined = Object.values(paths)
    .map((path) => read(path))
    .join('\n')
    .toLowerCase();
  for (const marker of ['ghp_', 'github_pat_', 'bearer ey', 'private_key-----']) {
    assert.ok(!combined.includes(marker), `credential marker found: ${marker}`);
  }
});
