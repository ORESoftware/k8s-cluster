import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function repoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/argocd/canonical-cloud/kustomization.yaml'))) {
      return candidate;
    }
  }
  throw new Error(`Unable to locate repository root from ${process.cwd()}`);
}

const root = repoRoot();
const overlay = 'remote/argocd/canonical-cloud';

async function read(relativePath: string): Promise<string> {
  return readFile(resolve(root, relativePath), 'utf8');
}

test('cluster GitOps owns the complete app.canonical.plus TLS boundary', async () => {
  const publicIngress = await read(`${overlay}/public.ingress.yaml`);
  const authIngress = await read(`${overlay}/shared-auth.ingress.yaml`);

  assert.match(publicIngress, /host: app\.canonical\.plus/);
  assert.match(authIngress, /host: app\.canonical\.plus/);
  assert.match(publicIngress, /secretName: canonical-plus-app-tls/);
  assert.match(authIngress, /secretName: canonical-plus-app-tls/);
  assert.doesNotMatch(authIngress, /cert-manager\.io\/cluster-issuer/);
  assert.match(authIngress, /path: \/shared-auth\(\/\|\$\)\(\.\*\)/);
  assert.match(authIngress, /pathType: ImplementationSpecific/);
  assert.match(authIngress, /nginx\.ingress\.kubernetes\.io\/rewrite-target: \/\$2/);
  assert.match(authIngress, /name: canonical-shared-auth-origin/);
  assert.match(authIngress, /number: 8120/);
});

test('the same-namespace ingress backend resolves only to the Canonical realm', async () => {
  const service = await read(`${overlay}/shared-auth.service.yaml`);
  const kustomization = await read(`${overlay}/kustomization.yaml`);

  assert.match(service, /type: ExternalName/);
  assert.match(
    service,
    /externalName: dd-shared-auth-canonical-plus\.shared-auth\.svc\.cluster\.local/,
  );
  assert.match(service, /port: 8120/);
  assert.match(kustomization, /- shared-auth\.service\.yaml/);
  assert.match(kustomization, /- shared-auth\.ingress\.yaml/);
});

test('API egress remains narrow while supporting identity, RDS, OTLP, and providers', async () => {
  const policy = await read(`${overlay}/api.networkpolicy.yaml`);

  assert.match(policy, /kubernetes\.io\/metadata\.name: kube-system/);
  assert.match(policy, /k8s-app: kube-dns/);
  assert.match(policy, /kubernetes\.io\/metadata\.name: shared-auth/);
  assert.match(policy, /app\.kubernetes\.io\/instance: canonical-plus/);
  assert.match(policy, /port: 8120/);
  assert.match(policy, /cidr: 172\.31\.0\.0\/16/);
  assert.match(policy, /port: 5432/);
  assert.match(policy, /port: 4317/);
  assert.match(policy, /cidr: 0\.0\.0\.0\/0/);
  assert.match(policy, /- 10\.0\.0\.0\/8/);
  assert.match(policy, /- 172\.16\.0\.0\/12/);
  assert.match(policy, /- 192\.168\.0\.0\/16/);
  assert.doesNotMatch(policy, /namespaceSelector: \{\}/);
  assert.doesNotMatch(policy, /port: 80\s*$/m);
});
