import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import test from 'node:test';

const root = resolve(import.meta.dirname, '../../..');
const read = (path) => readFileSync(resolve(root, path), 'utf8');

const files = {
  cloneDeployment:
    'remote/argocd/dd-next-runtime/dd-gha-clone-server.deployment.yaml',
  cloneNetworkPolicy:
    'remote/argocd/dd-next-runtime/dd-gha-clone-server.networkpolicy.yaml',
  routerDeployment:
    'remote/argocd/dd-next-runtime/dd-gha-executor-router.deployment.yaml',
  routerConfig:
    'remote/argocd/dd-next-runtime/dd-gha-executor-router.configmap.yaml',
  routerSecret:
    'remote/argocd/dd-next-runtime/dd-gha-executor-router.externalsecret.yaml',
  routerPolicy:
    'remote/argocd/dd-next-runtime/dd-gha-executor-router.networkpolicy.yaml',
  kustomization: 'remote/argocd/dd-next-runtime/kustomization.yaml',
  runbook: 'docs/operations/gha-clone-webhook-activation.md',
};

const zeroDigest =
  'sha256:0000000000000000000000000000000000000000000000000000000000000000';
const publishedRevision = '5aad32c37be7f29f9355f19d6ce6d316494ff141';
const publishedImages = {
  clone:
    'ghcr.io/oresoftware/gha-clone-server@sha256:44684171d909f96fe216d529bfc14f6f32a11e87c0f339d1877ac20606223c97',
  router:
    'ghcr.io/oresoftware/gha-executor-router@sha256:59a31a496e5c528f89acb7643b8ced1ea14bc6c15b1d83b22a37f4ba529708e6',
};

function requireAll(text, values, label) {
  for (const value of values) {
    assert.ok(text.includes(value), `${label} missing ${value}`);
  }
}

function literalEnv(text, name) {
  const match = text.match(
    new RegExp(
      `- name: ${name}\\n\\s+value: (?:(?:"([^"\\n]*)")|([^\\n#]+))`,
    ),
  );
  assert.ok(match, `${name} is missing or not a literal`);
  return (match[1] ?? match[2]).trim();
}

test('digest-pinned clone and router activate only the reviewed pilot', () => {
  const clone = read(files.cloneDeployment);
  const router = read(files.routerDeployment);

  for (const [label, deployment, binary, image] of [
    ['clone', clone, 'gha-clone-server', publishedImages.clone],
    ['router', router, 'gha-executor-router', publishedImages.router],
  ]) {
    requireAll(
      deployment,
      [
        'replicas: 1',
        image,
        publishedRevision,
        `command: ["/usr/local/bin/${binary}"]`,
        'automountServiceAccountToken: false',
        'readOnlyRootFilesystem: true',
        'allowPrivilegeEscalation: false',
        'drop: ["ALL"]',
        'signed-workflow-run-pilot',
      ],
      `${label} deployment`,
    );
    assert.ok(
      !deployment.includes(zeroDigest),
      `${label} deployment still uses the all-zero image sentinel`,
    );
    assert.doesNotMatch(
      deployment,
      /cargo run|git clone|git init|SOURCE_REF|SOURCE_URL|\/bin\/(?:ba)?sh/,
      `${label} deployment contains a source-build or shell bootstrap`,
    );
  }

  assert.equal(literalEnv(clone, 'GHA_CLONE_EXECUTION_ENABLED'), 'true');
  assert.equal(
    literalEnv(clone, 'GHA_CLONE_WEBHOOK_EXECUTION_ENABLED'),
    'true',
  );
  assert.equal(
    literalEnv(router, 'GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED'),
    'true',
  );
});

test('clone can address only the internal executor router', () => {
  const clone = read(files.cloneDeployment);
  assert.equal(
    literalEnv(clone, 'GHA_CLONE_BUILD_SERVER_URL'),
    'http://dd-gha-executor-router.default.svc.cluster.local:8126',
  );
  assert.ok(!clone.includes('dd-build-server.default.svc.cluster.local:8100'));
  requireAll(
    clone,
    [
      'name: GHA_CLONE_BUILD_SERVER_AUTH',
      'name: dd-gha-executor-router-secrets',
      'key: inbound_auth',
    ],
    'clone-to-router authentication',
  );
});

test('AWS is the only enabled independent executor and Hetzner is inert', () => {
  const config = read(files.routerConfig);
  const jsonText = config.split('GHA_EXECUTOR_ROUTER_EXECUTORS_JSON: |-\n')[1];
  assert.ok(jsonText, 'executor JSON literal is missing');
  const executors = JSON.parse(
    jsonText
      .split('\n')
      .map((line) => line.replace(/^    /, ''))
      .join('\n'),
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
});

test('router reuses the existing AWS authority without duplicating it', () => {
  const deployment = read(files.routerDeployment);
  const externalSecret = read(files.routerSecret);

  requireAll(
    deployment,
    [
      'projected:',
      'name: dd-gha-executor-router-secrets',
      'key: inbound_auth',
      'path: inbound-auth',
      'name: dd-agent-secrets',
      'key: SERVER_AUTH_SECRET',
      'path: aws-build-server-auth',
    ],
    'projected executor credentials',
  );
  assert.equal(
    (externalSecret.match(/secretKey:/g) ?? []).length,
    1,
    'router ExternalSecret must own only its inbound authority',
  );
  requireAll(
    externalSecret,
    ['secretKey: inbound_auth', 'property: inbound_auth'],
    'router ExternalSecret',
  );
  assert.doesNotMatch(
    externalSecret,
    /secretKey:\s*(?:aws_build_server_auth|hetzner)|property:\s*(?:aws_build_server_auth|hetzner)/i,
  );
});

test('disabled Hetzner has no public route or dormant credential surface', () => {
  const policy = read(files.routerPolicy);
  const config = read(files.routerConfig);
  assert.doesNotMatch(policy, /cidr:\s*0\.0\.0\.0\/0|port:\s*443/);
  const hetznerEntry = config.split('"provider": "hetzner"')[1] ?? '';
  assert.ok(hetznerEntry.includes('"enabled": false'));
  assert.ok(!hetznerEntry.includes('"url"'));
  assert.ok(!hetznerEntry.includes('"authPath"'));
});

test('public intake is exact-path and reaches only the clone server', () => {
  const route = read(files.cloneNetworkPolicy);
  requireAll(
    route,
    [
      'name: dd-gha-clone-server-webhook',
      'path: /gha-webhooks/github',
      'pathType: Exact',
      'nginx.ingress.kubernetes.io/rewrite-target: /webhooks/github',
      'nginx.ingress.kubernetes.io/proxy-request-buffering: "off"',
      'name: dd-gha-clone-server',
      'number: 8125',
      'kubernetes.io/metadata.name: ingress-nginx',
    ],
    'webhook ingress',
  );
  assert.ok(!route.includes('name: dd-gha-executor-router-webhook'));
  assert.ok(!route.includes('number: 8126'));
});

test('Argo tracks the complete active router surface', () => {
  const kustomization = read(files.kustomization);
  for (const filename of [
    'dd-gha-executor-router.configmap.yaml',
    'dd-gha-executor-router.externalsecret.yaml',
    'dd-gha-executor-router.deployment.yaml',
    'dd-gha-executor-router.service.yaml',
    'dd-gha-executor-router.networkpolicy.yaml',
    'dd-gha-clone-server.networkpolicy.yaml',
  ]) {
    assert.ok(kustomization.includes(`  - ${filename}`));
  }
});

test('runbook documents budget limits, live proof, status gap, and rollback', () => {
  const runbook = read(files.runbook);
  requireAll(
    runbook,
    [
      'digest-pinned',
      'workflow_run',
      'gha-capacity-broker-rs',
      'not activated by this change',
      'verify_gha_workflow_run_fallback.sh',
      'HTTP 401',
      'does not yet',
      'Rollback',
      'replicas: 0',
    ],
    'activation runbook',
  );
});
