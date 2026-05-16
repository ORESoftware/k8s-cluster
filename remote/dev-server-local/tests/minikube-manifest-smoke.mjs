import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';

const packageRoot = resolve(new URL('..', import.meta.url).pathname);
const manifestPath = join(packageRoot, 'k8s', 'minikube-dev-server.yaml');
const manifest = readFileSync(manifestPath, 'utf8');

const requiredFragments = [
  'kind: Namespace',
  'name: dd-dev-local',
  'pod-security.kubernetes.io/enforce: baseline',
  'pod-security.kubernetes.io/warn: baseline',
  'kind: ServiceAccount',
  'automountServiceAccountToken: false',
  'kind: ConfigMap',
  'name: dd-dev-server-local-config',
  'kind: Secret',
  'name: dd-dev-server-local-secrets',
  'kind: PersistentVolumeClaim',
  'name: dd-dev-server-local-workspace',
  'kind: ResourceQuota',
  'name: dd-dev-server-local-quota',
  'requests.ephemeral-storage: "20Gi"',
  'limits.ephemeral-storage: "40Gi"',
  'kind: Deployment',
  'name: dd-dev-server-local',
  'serviceAccountName: dd-dev-server-local',
  'image: dd-dev-server-local:latest',
  'imagePullPolicy: Never',
  'ephemeral-storage: "4Gi"',
  'ephemeral-storage: "8Gi"',
  'REMOTE_DEV_THREAD_ID: "00000000-0000-4000-8000-000000000001"',
  'kind: Service',
  'type: ClusterIP',
  'kind: NetworkPolicy',
  'name: dd-dev-server-local-policy',
  'kubernetes.io/metadata.name: kube-system',
  'cidr: 0.0.0.0/0',
  '169.254.0.0/16',
];

for (const fragment of requiredFragments) {
  assert(
    manifest.includes(fragment),
    `expected minikube manifest to include ${JSON.stringify(fragment)}`,
  );
}

assert(
  !manifest.includes('kind: Ingress'),
  'local minikube manifest should not own production EC2 ingress resources',
);

const dryRun = spawnSync(
  'kubectl',
  ['create', '--dry-run=client', '--validate=false', '-f', manifestPath],
  {
    cwd: packageRoot,
    encoding: 'utf8',
  },
);

const kubectlClusterUnavailable =
  dryRun.status !== 0 &&
  /connect: connection refused|couldn't get current server API group list|Kubernetes cluster unreachable|no configuration has been provided/i.test(
    dryRun.stderr,
  );

if (kubectlClusterUnavailable) {
  console.warn('kubectl dry-run skipped because the local minikube API server is unavailable');
} else {
  assert.equal(
    dryRun.status,
    0,
    `kubectl dry-run failed\nstdout:\n${dryRun.stdout}\nstderr:\n${dryRun.stderr}`,
  );

  for (const resource of [
    'namespace/dd-dev-local',
    'serviceaccount/dd-dev-server-local',
    'configmap/dd-dev-server-local-config',
    'secret/dd-dev-server-local-secrets',
    'persistentvolumeclaim/dd-dev-server-local-workspace',
    'resourcequota/dd-dev-server-local-quota',
    'deployment.apps/dd-dev-server-local',
    'service/dd-dev-server-local',
    'networkpolicy.networking.k8s.io/dd-dev-server-local-policy',
  ]) {
    assert(dryRun.stdout.includes(resource), `kubectl dry-run did not include ${resource}`);
  }
}

console.log('remote/dev-server-local minikube manifest smoke passed');
