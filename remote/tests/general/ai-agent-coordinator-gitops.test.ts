import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/argocd/projects/_template.appproject.yaml'))) {
      return candidate;
    }
  }
  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();
const read = (path: string) => readFile(resolve(repoRoot, path), 'utf8');
const tenantPath = 'remote/argocd/projects/ai-agent-coordinator.tenant.yaml';
const projectPath = 'remote/argocd/projects/ai-agent-coordinator.appproject.yaml';
const applicationPath = 'remote/argocd/apps/ai-agent-coordinator.application.yaml';
const bootstrapApplicationPath =
  'remote/argocd/apps/ai-agent-coordinator-repository-bootstrap.application.yaml';
const appCommit = '83091cb3b3dc1fc4797e822f7fe2d320ca1c3cd9';
const bootstrapCommit = '11bf6edff185d72c8f606be1c3fcc485a3ee4695';

test('coordinator tenant owns the namespace guardrails and identity', async () => {
  const tenant = await read(tenantPath);
  assert.match(tenant, /kind: Namespace\s+metadata:\s+name: ai-agent-coordinator/s);
  assert.match(tenant, /kind: ResourceQuota/);
  assert.match(tenant, /requests\.storage: 10Gi/);
  assert.match(tenant, /persistentvolumeclaims: "2"/);
  assert.match(tenant, /kind: LimitRange/);
  assert.match(
    tenant,
    /kind: ServiceAccount\s+metadata:\s+name: ai-agent-coordinator\s+namespace: ai-agent-coordinator/s,
  );
  assert.match(tenant, /app\.kubernetes\.io\/part-of: oresoftware-agent-platform/);
  assert.match(tenant, /automountServiceAccountToken: false/);
  assert.match(tenant, /name: ai-agent-coordinator-default-deny-ingress/);
  assert.match(tenant, /policyTypes: \[Ingress\]/);
});

test('coordinator AppProject admits one source and no cluster resources', async () => {
  const project = await read(projectPath);
  assert.match(project, /name: ai-agent-coordinator\s+namespace: argocd/);
  assert.match(
    project,
    /sourceRepos:\s+- "https:\/\/github\.com\/ORESoftware\/ai-agent-coordinator\.rs\.git"/,
  );
  assert.match(project, /namespace: ai-agent-coordinator/);
  assert.match(project, /clusterResourceWhitelist: \[\]/);
  for (const kind of ['ResourceQuota', 'LimitRange', 'ServiceAccount']) {
    assert.match(project, new RegExp(`kind: ${kind}`));
  }
  assert.doesNotMatch(project, /kind: NetworkPolicy/);
  assert.doesNotMatch(project, /k8s-cluster\.git/);
});

test('Argo tracks the reviewed app repo commit and deploy directory', async () => {
  const application = await read(applicationPath);
  assert.match(application, /name: dd-ai-agent-coordinator/);
  assert.match(application, /project: ai-agent-coordinator/);
  assert.match(
    application,
    /repoURL: https:\/\/github\.com\/ORESoftware\/ai-agent-coordinator\.rs\.git/,
  );
  assert.match(application, new RegExp(`targetRevision: ${appCommit}`));
  assert.doesNotMatch(application, /targetRevision: (?:main|master|HEAD)/);
  assert.match(application, /path: deploy\/k8s/);
  assert.match(application, /namespace: ai-agent-coordinator/);
  assert.match(application, /prune: true/);
  assert.match(application, /selfHeal: true/);
});

test('Argo pins the one-shot repository bootstrap to its reviewed commit and path', async () => {
  const application = await read(bootstrapApplicationPath);
  assert.match(application, /name: dd-ai-agent-coordinator-repository-bootstrap/);
  assert.match(application, /project: ai-agent-coordinator/);
  assert.match(
    application,
    /repoURL: https:\/\/github\.com\/ORESoftware\/ai-agent-coordinator\.rs\.git/,
  );
  assert.match(application, new RegExp(`targetRevision: ${bootstrapCommit}`));
  assert.doesNotMatch(application, /targetRevision: (?:main|master|HEAD)/);
  assert.match(application, /path: deploy\/k8s\/bootstrap/);
  assert.match(application, /namespace: ai-agent-coordinator/);
  assert.match(application, /linear\.app\/issue: DEN-877/);
  assert.match(application, /prune: true/);
  assert.match(application, /selfHeal: true/);
  assert.match(application, /ServerSideApply=true/);
});

test('registration files are free of unresolved merge markers', async () => {
  const files = await Promise.all(
    [tenantPath, projectPath, applicationPath, bootstrapApplicationPath].map(read),
  );
  for (const contents of files) {
    assert.doesNotMatch(contents, /^(?:<<<<<<<|=======|>>>>>>>)/m);
  }
});
