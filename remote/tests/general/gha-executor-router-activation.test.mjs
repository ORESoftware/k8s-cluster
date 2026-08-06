import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import test from 'node:test';

const root = resolve(import.meta.dirname, '../../..');
const read = (path) => readFileSync(resolve(root, path), 'utf8');

const files = {
  cloneDeployment:
    'remote/argocd/dd-next-runtime/dd-gha-clone-server.deployment.yaml',
  routerDeployment:
    'remote/argocd/dd-next-runtime/dd-gha-executor-router.deployment.yaml',
  routerConfig:
    'remote/argocd/dd-next-runtime/dd-gha-executor-router.configmap.yaml',
  routerSecret:
    'remote/argocd/dd-next-runtime/dd-gha-executor-router.externalsecret.yaml',
  routerPolicy:
    'remote/argocd/dd-next-runtime/dd-gha-executor-router.networkpolicy.yaml',
  kustomization: 'remote/argocd/dd-next-runtime/kustomization.yaml',
  runbook: 'docs/gha-executor-router-activation.md',
};

const zeroDigest =
  'sha256:0000000000000000000000000000000000000000000000000000000000000000';

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

test('clone and router cannot be activated by merging the scaffold', () => {
  const clone = read(files.cloneDeployment);
  const router = read(files.routerDeployment);

  for (const [label, deployment, binary] of [
    ['clone', clone, 'gha-clone-server'],
    ['router', router, 'gha-executor-router'],
  ]) {
    requireAll(
      deployment,
      [
        'replicas: 0',
        zeroDigest,
        `command: ["/usr/local/bin/${binary}"]`,
        'automountServiceAccountToken: false',
        'readOnlyRootFilesystem: true',
        'allowPrivilegeEscalation: false',
        'drop: ["ALL"]',
      ],
      `${label} deployment`,
    );
    assert.doesNotMatch(
      deployment,
      /cargo run|git clone|git init|SOURCE_REF|SOURCE_URL|\/bin\/(?:ba)?sh/,
      `${label} deployment contains a source-build or shell bootstrap`,
    );
  }

  assert.equal(literalEnv(clone, 'GHA_CLONE_EXECUTION_ENABLED'), 'false');
  assert.equal(
    literalEnv(clone, 'GHA_CLONE_WEBHOOK_EXECUTION_ENABLED'),
    'false',
  );
  assert.equal(
    literalEnv(router, 'GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED'),
    'false',
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

test('Argo tracks the complete inert router surface', () => {
  const kustomization = read(files.kustomization);
  for (const filename of [
    'dd-gha-executor-router.configmap.yaml',
    'dd-gha-executor-router.externalsecret.yaml',
    'dd-gha-executor-router.deployment.yaml',
    'dd-gha-executor-router.service.yaml',
    'dd-gha-executor-router.networkpolicy.yaml',
  ]) {
    assert.ok(kustomization.includes(`  - ${filename}`));
  }
});

test('runbook requires immutable images, provider proof, and rollback', () => {
  const runbook = read(files.runbook);
  requireAll(
    runbook,
    [
      'digest-pinned',
      'SBOM',
      'AWS',
      'Hetzner',
      'pre-submit',
      'Fiducia',
      'Rollback',
      'replicas: 0',
    ],
    'activation runbook',
  );
});
