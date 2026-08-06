import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/argocd/project-automation/kustomization.yaml'))) {
      return candidate;
    }
  }
  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();
const overlay = 'remote/argocd/project-automation';
const releaseSha = 'd31d817cee22cc5cd6d4473a239012f38d16fbe2';

async function readRepoFile(path: string): Promise<string> {
  return readFile(resolve(repoRoot, path), 'utf8');
}

test('project automation is Argo managed from dev and fail-closed before activation', async () => {
  const application = await readRepoFile(
    'remote/argocd/clusters/hetzner/project-automation.application.yaml',
  );
  const clusterKustomization = await readRepoFile(
    'remote/argocd/clusters/hetzner/kustomization.yaml',
  );
  const deployment = await readRepoFile(`${overlay}/deployment.yaml`);

  assert.match(application, /kind: Application/);
  assert.match(application, /name: dd-project-automation/);
  assert.match(application, /targetRevision: dev/);
  assert.match(application, /path: remote\/argocd\/project-automation/);
  assert.match(application, /automated:\s*\n\s*prune: true\s*\n\s*selfHeal: true/);
  assert.match(clusterKustomization, /project-automation\.application\.yaml/);
  assert.match(deployment, /replicas: 0/);
  assert.doesNotMatch(deployment, /replicas: [1-9]/);
});

test('deployment uses the immutable reviewed image and no PAT-shaped secret', async () => {
  const deployment = await readRepoFile(`${overlay}/deployment.yaml`);
  const secret = await readRepoFile(`${overlay}/externalsecret.yaml`);

  assert.match(
    deployment,
    new RegExp(`image: ghcr\\.io/oresoftware/project-automation:${releaseSha}`),
  );
  assert.doesNotMatch(deployment, /:(?:main|latest)$/m);
  assert.match(deployment, /automountServiceAccountToken: false/);
  assert.match(deployment, /readOnlyRootFilesystem: true/);
  assert.match(deployment, /allowPrivilegeEscalation: false/);
  assert.match(deployment, /capabilities:\s*\n\s*drop:\s*\n\s*- ALL/);
  assert.match(deployment, /startupProbe:[\s\S]*path: \/healthz/);
  assert.match(deployment, /readinessProbe:[\s\S]*path: \/healthz/);
  assert.match(deployment, /livenessProbe:[\s\S]*path: \/healthz/);

  assert.match(secret, /key: dd\/remote-dev\/project-automation-secrets/);
  assert.match(secret, /property: github_app_private_key/);
  assert.match(secret, /property: linear_api_key/);
  assert.doesNotMatch(`${deployment}\n${secret}`, /github_pat_|ghp_|personal.?access.?token/i);
});

test('public webhook exposure and egress are narrowly bounded', async () => {
  const ingress = await readRepoFile(`${overlay}/ingress.yaml`);
  const service = await readRepoFile(`${overlay}/service.yaml`);
  const policy = await readRepoFile(`${overlay}/networkpolicy.yaml`);

  assert.match(ingress, /project-automation\.95-217-171-250\.sslip\.io/);
  assert.match(ingress, /path: \/webhooks\/github/);
  assert.match(ingress, /path: \/healthz/);
  assert.match(service, /type: ClusterIP/);
  assert.match(service, /port: 8787/);
  assert.match(service, /targetPort: http/);
  assert.match(policy, /kubernetes\.io\/metadata\.name: ingress-nginx/);
  assert.match(policy, /port: 8787/);
  assert.match(policy, /kubernetes\.io\/metadata\.name: kube-system/);
  assert.match(policy, /port: 53/);
  assert.match(policy, /port: 443/);
  assert.doesNotMatch(policy, /port: 80\s*$/m);
});
