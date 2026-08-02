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
  throw new Error(`Unable to locate repository root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();
const read = (path: string) => readFile(resolve(repoRoot, path), 'utf8');
const appCommit = 'd25e04e50be4a9fad039cfcfa6c321e9c99a1e02';
const platformRoot = 'remote/argocd/projects/ai-agent-coordinator';
const tenantPath = `${platformRoot}/tenant.yaml`;
const projectPath = `${platformRoot}/appproject.yaml`;
const platformKustomizationPath = `${platformRoot}/kustomization.yaml`;
const platformReadmePath = `${platformRoot}/README.md`;
const platformApplicationPath = 'remote/argocd/apps/ai-agent-coordinator-platform.application.yaml';
const workloadApplicationPath = 'remote/argocd/apps/ai-agent-coordinator.application.yaml';
const clouds = ['aws', 'gcp', 'hetzner'] as const;

function assertImmutablePilotApplication(application: string): void {
  assert.match(application, /name: ai-agent-coordinator\s+namespace: argocd/s);
  assert.match(application, /project: ai-agent-coordinator/);
  assert.match(application, /repoURL: https:\/\/github\.com\/ORESoftware\/ai-agent-coordinator\.rs\.git/);
  assert.match(application, new RegExp(`targetRevision: ${appCommit}`));
  assert.match(application, /path: deploy\/overlays\/cross-org-linear-pilot/);
  assert.match(application, /namespace: ai-agent-coordinator/);
  assert.match(application, /argocd\.argoproj\.io\/sync-wave: "0"/);
  assert.match(application, /ServerSideApply=true/);
  assert.match(application, /PruneLast=true/);
  assert.doesNotMatch(application, /targetRevision: (?:main|master|HEAD)/);
  assert.doesNotMatch(application, /path: deploy\/k8s(?:\s|$)/);
  assert.doesNotMatch(application, /CreateNamespace=true/);
}

function assertPlatformApplication(application: string): void {
  assert.match(application, /name: ai-agent-coordinator-platform\s+namespace: argocd/s);
  assert.match(application, /project: default/);
  assert.match(application, /repoURL: https:\/\/github\.com\/ORESoftware\/k8s-cluster\.git/);
  assert.match(application, /targetRevision: dev/);
  assert.match(application, /path: remote\/argocd\/projects\/ai-agent-coordinator/);
  assert.match(application, /argocd\.argoproj\.io\/sync-wave: "-1"/);
  assert.match(application, /namespace: argocd/);
}

test('platform tenant owns namespace guardrails and identity', async () => {
  const tenant = await read(tenantPath);
  assert.match(tenant, /kind: Namespace\s+metadata:\s+name: ai-agent-coordinator/s);
  assert.match(tenant, /pod-security\.kubernetes\.io\/enforce: baseline/);
  assert.match(tenant, /kind: ResourceQuota/);
  assert.match(tenant, /requests\.storage: 10Gi/);
  assert.match(tenant, /persistentvolumeclaims: "2"/);
  assert.match(tenant, /pods: "20"/);
  assert.match(tenant, /kind: LimitRange/);
  assert.match(tenant, /kind: ServiceAccount\s+metadata:\s+name: ai-agent-coordinator\s+namespace: ai-agent-coordinator/s);
  assert.match(tenant, /automountServiceAccountToken: false/);
  assert.match(tenant, /name: ai-agent-coordinator-default-deny-ingress/);
  assert.match(tenant, /policyTypes:\s+- Ingress/);
  assert.doesNotMatch(tenant, /kind: Secret/);
});

test('strict AppProject allows one source, one namespace, and no cluster resources', async () => {
  const project = await read(projectPath);
  assert.match(project, /name: ai-agent-coordinator\s+namespace: argocd/);
  assert.match(project, /sourceRepos:\s+- "https:\/\/github\.com\/ORESoftware\/ai-agent-coordinator\.rs\.git"/);
  assert.match(project, /destinations:\s+- server: https:\/\/kubernetes\.default\.svc\s+namespace: ai-agent-coordinator/s);
  assert.match(project, /clusterResourceWhitelist: \[\]/);
  for (const kind of ['ResourceQuota', 'LimitRange', 'ServiceAccount']) {
    assert.match(project, new RegExp(`kind: ${kind}`));
  }
  assert.doesNotMatch(project, /kind: NetworkPolicy/);
  assert.doesNotMatch(project, /k8s-cluster\.git/);
});

test('platform bundle is self-contained and composes only tenant resources plus AppProject', async () => {
  const kustomization = await read(platformKustomizationPath);
  assert.match(kustomization, /- tenant\.yaml/);
  assert.match(kustomization, /- appproject\.yaml/);
  assert.doesNotMatch(kustomization, /\.\.\//);
  assert.doesNotMatch(kustomization, /deployment|service|externalsecret/i);

  const readme = await read(platformReadmePath);
  assert.match(readme, /self-contained/);
  assert.match(readme, /default load restriction/);
});

test('canonical Applications separate platform and immutable workload ownership', async () => {
  assertPlatformApplication(await read(platformApplicationPath));
  assertImmutablePilotApplication(await read(workloadApplicationPath));
});

test('AWS, GCP, and Hetzner cluster roots include equivalent coordinator registrations', async () => {
  for (const cloud of clouds) {
    const kustomization = await read(`remote/argocd/clusters/${cloud}/kustomization.yaml`);
    assert.match(kustomization, /- ai-agent-coordinator\.applications\.yaml/);

    const applications = await read(`remote/argocd/clusters/${cloud}/ai-agent-coordinator.applications.yaml`);
    assert.equal((applications.match(/^kind: Application$/gm) ?? []).length, 2, `${cloud} must register exactly two Applications`);
    assertPlatformApplication(applications);
    assertImmutablePilotApplication(applications);
    assert.doesNotMatch(applications, /CreateNamespace=true/);
  }
});

test('registration files contain no credentials, plaintext Secrets, or merge markers', async () => {
  const paths = [
    tenantPath,
    projectPath,
    platformKustomizationPath,
    platformReadmePath,
    platformApplicationPath,
    workloadApplicationPath,
    ...clouds.flatMap((cloud) => [
      `remote/argocd/clusters/${cloud}/kustomization.yaml`,
      `remote/argocd/clusters/${cloud}/ai-agent-coordinator.applications.yaml`,
    ]),
  ];
  const files = await Promise.all(paths.map(read));
  for (const contents of files) {
    assert.doesNotMatch(contents, /^(?:<<<<<<<|=======|>>>>>>>)/m);
    assert.doesNotMatch(contents, /^kind:\s*Secret\s*$/m);
    assert.doesNotMatch(contents, /gh[pousr]_[A-Za-z0-9_]{20,}/);
    assert.doesNotMatch(contents, /sk-[A-Za-z0-9_-]{16,}/);
    assert.doesNotMatch(contents, /LINEAR_API_TOKEN\s*[:=]\s*[^\s#]+/);
  }
});
