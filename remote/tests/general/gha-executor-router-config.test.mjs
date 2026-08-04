import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '../../..');
const read = (path) => readFileSync(resolve(root, path), 'utf8');
const testedRouterSha = '6146668400441de15a8d8e9f513786096db9a730';

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
  routerStartupTests:
    'remote/deployments/gha-clone-server-rs/tests/executor_router_startup_security.rs',
  architecture: 'docs/gha-continuity-architecture.md',
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

test('executor inventory is ordered, exact, and carries no dormant Hetzner endpoint', () => {
  const executors = JSON.parse(
    extractLiteralBlock(
      read(paths.configMap),
      'GHA_EXECUTOR_ROUTER_EXECUTORS_JSON',
    ),
  );
  assert.deepEqual(executors, [
    {
      id: 'aws-primary',
      provider: 'aws',
      enabled: true,
      url: 'http://dd-build-server.default.svc.cluster.local:8100',
      authPath:
        '/var/run/secrets/gha-executor-router/aws-build-server-auth',
    },
    {
      id: 'hetzner-secondary',
      provider: 'hetzner',
      enabled: false,
    },
  ]);
  assert.equal(new Set(executors.map(({ id }) => id)).size, executors.length);
});

test('router remains absent, disabled, and pinned to the tested source commit', () => {
  const deployment = read(paths.deployment);
  requireContains(
    deployment,
    [
      'name: dd-gha-executor-router',
      'replicas: 0',
      'type: Recreate',
      'automountServiceAccountToken: false',
      'name: GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED',
      'value: "false"',
      'name: GHA_EXECUTOR_ROUTER_SOURCE_SHA',
      `value: ${testedRouterSha}`,
      'fetch --depth 1 --no-tags origin "${GHA_EXECUTOR_ROUTER_SOURCE_SHA}"',
      'checkout --detach FETCH_HEAD',
      'rev-parse HEAD',
      'cargo run --locked --release --bin gha-executor-router',
      'drop: ["ALL"]',
      'path: /readyz',
      'path: /healthz',
    ],
    'router deployment',
  );
  assert.equal(
    envValue(deployment, 'GHA_EXECUTOR_ROUTER_SOURCE_SHA'),
    testedRouterSha,
  );
  assert.equal(
    envValue(deployment, 'GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED'),
    'false',
  );
  assert.doesNotMatch(deployment, /GHA_EXECUTOR_ROUTER_SOURCE_REF|--branch/);
  assert.doesNotMatch(deployment, /value:\s*(?:ghp_|github_pat_)/i);
});

test('router projects one inbound authority and reuses the existing AWS build authority', () => {
  const deployment = read(paths.deployment);
  requireContains(
    deployment,
    [
      'projected:',
      'name: dd-gha-executor-router-secrets',
      'key: router_auth',
      'path: router-auth',
      'name: dd-agent-secrets',
      'key: SERVER_AUTH_SECRET',
      'path: aws-build-server-auth',
      'mountPath: /var/run/secrets/gha-executor-router',
      'readOnly: true',
    ],
    'router projected secret volume',
  );
  assert.doesNotMatch(deployment, /key:\s*aws_build_server_auth/);

  const externalSecret = read(paths.externalSecret);
  requireContains(
    externalSecret,
    [
      'key: dd/remote-dev/gha-executor-router-secrets',
      'secretKey: router_auth',
      'property: router_auth',
    ],
    'router ExternalSecret',
  );
  assert.equal(
    (externalSecret.match(/secretKey:/g) ?? []).length,
    1,
    'router ExternalSecret introduced a duplicate executor authority',
  );
  assert.doesNotMatch(externalSecret, /aws_build_server_auth|hetzner/i);
  assert.doesNotMatch(
    externalSecret,
    /ghp_|github_pat_|stringData:|BEGIN (?:RSA |EC )?PRIVATE KEY/i,
  );
});

test('clone server routes only to the internal router and names its binary explicitly', () => {
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
      'name: GHA_CLONE_SOURCE_SHA',
      `value: ${testedRouterSha}`,
      'cargo run --locked --release --bin gha-clone-server',
    ],
    'clone server deployment',
  );
  assert.equal(envValue(deployment, 'GHA_CLONE_EXECUTION_ENABLED'), 'false');
  assert.equal(
    envValue(deployment, 'GHA_CLONE_WEBHOOK_EXECUTION_ENABLED'),
    'false',
  );
  assert.equal(envValue(deployment, 'GHA_CLONE_SOURCE_SHA'), testedRouterSha);
  assert.doesNotMatch(deployment, /GHA_CLONE_SOURCE_REF|--branch/);
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
    'clone server ExternalSecret',
  );
});

test('network policies permit only clone-to-router and router-to-AWS paths before Hetzner activation', () => {
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
      'No generic Internet egress exists in the inert review scaffold.',
    ],
    'router NetworkPolicy',
  );
  assert.doesNotMatch(routerPolicy, /cidr:\s*0\.0\.0\.0\/0/);
  assert.doesNotMatch(routerPolicy, /port:\s*443/);

  const clonePolicy = read(paths.cloneNetworkPolicy);
  requireContains(
    clonePolicy,
    ['app: dd-gha-executor-router', 'port: 8126'],
    'clone server NetworkPolicy',
  );
  const egress = clonePolicy.split('  egress:\n')[1] ?? '';
  assert.ok(!egress.includes('port: 8100'), 'clone server retains direct build egress');
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
    assert.ok(
      kustomization.includes(`  - ${filename}`),
      `kustomization missing ${filename}`,
    );
  }
});

test('router code and process tests retain the no-duplicate and source-redaction boundaries', () => {
  const source = read(paths.routerSource);
  const contracts = read(paths.routerContracts);
  const tests = read(paths.routerTests);
  const startupTests = read(paths.routerStartupTests);
  requireContains(
    source,
    [
      'automatic provider failover is blocked to prevent duplicate work',
      'first_ready_executor',
      'namespace_build_id',
      'parse_namespaced_build_id',
      'requestIdForwardedUnchanged',
      'postSubmissionFailover',
      'shared Fiducia-fenced claim',
      '.redirect(reqwest::redirect::Policy::none())',
      "value.contains('\\n')",
      "value.contains('\\r')",
    ],
    'router source',
  );
  requireContains(
    contracts,
    [
      'jobKind must be run-profile',
      'gitRef must be a lowercase 40-hex commit SHA',
      'disabled executors must omit url and authPath',
      'plain HTTP is allowed only for loopback or in-cluster',
      "auth.contains('\\n')",
      "auth.contains('\\r')",
      'rejects_multiline_executor_secret_files',
    ],
    'router contracts',
  );
  requireContains(
    tests,
    [
      'readiness_failure_routes_to_hetzner_before_any_submission',
      'ambiguous_submission_never_fails_over_or_leaks_upstream_body',
      'accepted_build_status_failure_remains_pinned_without_resubmission',
      'explicit_rejection_does_not_submit_to_the_second_provider',
    ],
    'router live tests',
  );
  requireContains(
    startupTests,
    [
      'multiline_inbound_router_secret_exits_before_binding_without_leaking',
      'multiline_executor_secret_exits_before_binding_without_leaking',
      'router did not reject the malformed mounted secret before binding',
    ],
    'router startup security tests',
  );
});

test('continuity workflow exercises router contracts and renders the complete overlay', () => {
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

test('architecture keeps native ARC parity separate from the bounded independent lane', () => {
  const architecture = read(paths.architecture);
  requireContains(
    architecture,
    [
      'Lane A: native parity through ARC',
      'Lane B: independent workflow compatibility',
      'gha-executor-router',
      'pre-submit',
      'Fiducia',
      'replicas: 0',
    ],
    'continuity architecture',
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
