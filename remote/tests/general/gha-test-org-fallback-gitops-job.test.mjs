import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
const read = (relative) => fs.readFileSync(path.join(root, relative), 'utf8');

const awsKustomization = read('remote/argocd/clusters/aws/kustomization.yaml');
const hetznerKustomization = read('remote/argocd/clusters/hetzner/kustomization.yaml');
const application = read(
  'remote/argocd/clusters/aws/gha-test-fallback-activation.application.yaml',
);
const activationKustomization = read(
  'remote/argocd/gha-test-fallback-activation/aws/kustomization.yaml',
);
const bootstrap = read('remote/argocd/gha-test-fallback-activation/aws/bootstrap.py');
const rbac = read('remote/argocd/gha-test-fallback-activation/aws/rbac.yaml');
const networkPolicy = read(
  'remote/argocd/gha-test-fallback-activation/aws/networkpolicy.yaml',
);
const job = read('remote/argocd/gha-test-fallback-activation/aws/job.yaml');

test('activation is reconciled only by the AWS root application', () => {
  assert.match(awsKustomization, /gha-test-fallback-activation\.application\.yaml/);
  assert.doesNotMatch(hetznerKustomization, /gha-test-fallback-activation/);
  assert.match(application, /targetRevision: dev/);
  assert.match(application, /path: remote\/argocd\/gha-test-fallback-activation\/aws/);
  assert.match(application, /prune: true/);
  assert.match(application, /selfHeal: true/);
});

test('one-shot job runs immutable reviewed programs without manifest credentials', () => {
  assert.match(activationKustomization, /configMapGenerator:/);
  assert.match(bootstrap, /TRUSTED_SHA = "04cfc4658f715d6ac77b9c16445add31ad0a761a"/);
  assert.match(
    bootstrap,
    /ACTIVATOR_SHA256 = "36d0e65154c03132fa6bd9491834629aa8883c28a1333e36ebdec054a22d9589"/,
  );
  assert.match(
    bootstrap,
    /KUBECTL_SHA256 = "8791ec7c8966b61420d55103a5fb948de9f0ca3d7306d789734975ad9704bdb0"/,
  );
  assert.match(bootstrap, /PUBLIC_IP_URL = "https:\/\/checkip\.amazonaws\.com\/"/);
  assert.match(job, /backoffLimit: 0/);
  assert.match(job, /automountServiceAccountToken: false/);
  assert.match(job, /serviceAccountToken:/);
  assert.match(job, /readOnlyRootFilesystem: true/);
  assert.match(job, /allowPrivilegeEscalation: false/);
  assert.match(job, /drop:\s+\- ALL/);
  assert.match(job, /python:3\.12-slim@sha256:[0-9a-f]{64}/);
  assert.doesNotMatch(job, /secretKeyRef|GH_PAT|github_webhook_secret|auth_secret/);
});

test('activation identity is read-only and names every credential-bearing resource', () => {
  assert.match(rbac, /resources: \["secrets"\]/);
  assert.match(rbac, /- dd-agent-secrets/);
  assert.match(rbac, /- dd-gha-clone-server-secrets/);
  assert.match(rbac, /- dd-gha-executor-router-secrets/);
  assert.doesNotMatch(rbac, /verbs:.*(?:list|watch|create|update|patch|delete)/);
  assert.equal((rbac.match(/verbs: \["get"\]/g) ?? []).length, 6);
  assert.match(networkPolicy, /ingress: \[\]/);
  assert.match(networkPolicy, /cidr: 10\.96\.0\.1\/32/);
  assert.match(networkPolicy, /app: dd-gha-clone-server/);
  assert.match(networkPolicy, /port: 8125/);
});
