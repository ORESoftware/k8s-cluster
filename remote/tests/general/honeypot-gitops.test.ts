import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readdir, readFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/argocd/projects/honeypot.tenant.yaml'))) {
      return candidate;
    }
  }
  throw new Error(`Unable to locate repository root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();
const read = (path: string) => readFile(resolve(repoRoot, path), 'utf8');
const tenantPath = 'remote/argocd/projects/honeypot.tenant.yaml';
const projectPath = 'remote/argocd/projects/honeypot.appproject.yaml';
const applicationPath = 'remote/argocd/apps/honeypot.application.yaml';

async function readTree(path: string): Promise<Array<{ path: string; contents: string }>> {
  const absolute = resolve(repoRoot, path);
  if (!existsSync(absolute)) return [];

  const output: Array<{ path: string; contents: string }> = [];
  for (const entry of await readdir(absolute, { withFileTypes: true })) {
    const relative = join(path, entry.name);
    if (entry.isDirectory()) output.push(...await readTree(relative));
    if (entry.isFile()) output.push({ path: relative, contents: await read(relative) });
  }
  return output;
}

test('platform tenant owns strict namespace guardrails and service identity', async () => {
  const tenant = await read(tenantPath);
  assert.match(tenant, /kind: Namespace\s+metadata:\s+name: honeypot/s);
  for (const mode of ['enforce', 'audit', 'warn']) {
    assert.match(tenant, new RegExp(`pod-security\\.kubernetes\\.io/${mode}: restricted`));
  }
  assert.match(tenant, /kind: ResourceQuota/);
  assert.match(tenant, /pods: "4"/);
  assert.match(tenant, /persistentvolumeclaims: "0"/);
  assert.match(tenant, /kind: LimitRange/);
  assert.match(tenant, /kind: ServiceAccount\s+metadata:\s+name: honeypot\s+namespace: honeypot\s+automountServiceAccountToken: false/s);
  assert.match(tenant, /name: honeypot-default-deny/);
  assert.match(tenant, /policyTypes:\s+- Ingress\s+- Egress/s);
  assert.doesNotMatch(tenant, /^kind:\s*Secret\s*$/m);
});

test('strict AppProject allows one source and one namespace, with no cluster resources', async () => {
  const project = await read(projectPath);
  assert.match(project, /name: honeypot\s+namespace: argocd/);
  assert.match(project, /sourceRepos:\s+- "git@github\.com:ORESoftware\/honeypot\.rs\.git"/);
  assert.match(project, /destinations:\s+- server: https:\/\/kubernetes\.default\.svc\s+namespace: honeypot/s);
  assert.match(project, /clusterResourceWhitelist: \[\]/);
  for (const kind of ['ResourceQuota', 'LimitRange', 'ServiceAccount', 'Role', 'RoleBinding', 'Secret']) {
    assert.match(project, new RegExp(`kind: ${kind}`));
  }
  assert.doesNotMatch(project, /k8s-cluster\.git/);
});

test('workload registration is immutable, direct, and explicitly gated', async () => {
  const application = await read(applicationPath);
  assert.match(application, /REVIEW-ONLY \/ INERT/);
  assert.match(application, /name: honeypot\s+namespace: argocd/);
  assert.match(application, /project: honeypot/);
  assert.match(application, /repoURL: git@github\.com:ORESoftware\/honeypot\.rs\.git/);
  assert.match(application, /targetRevision: v0\.1\.0/);
  assert.match(application, /path: deploy\/k8s/);
  assert.match(application, /namespace: honeypot/);
  assert.match(application, /ServerSideApply=true/);
  assert.match(application, /PruneLast=true/);
  assert.doesNotMatch(application, /targetRevision: (?:main|master|HEAD)/);
  assert.doesNotMatch(application, /CreateNamespace=true/);
});

test('honeypot registration remains absent from every live cluster root', async () => {
  const files = await readTree('remote/argocd/clusters');
  for (const file of files) {
    assert.doesNotMatch(
      file.contents,
      /(?:honeypot\.application\.yaml|name:\s*honeypot\b|ORESoftware\/honeypot\.rs)/,
      `${file.path} must not activate the review-only honeypot`,
    );
  }
});

test('platform files contain no credentials, plaintext Secrets, or merge markers', async () => {
  const files = await Promise.all([tenantPath, projectPath, applicationPath].map(read));
  for (const contents of files) {
    assert.doesNotMatch(contents, /^(?:<<<<<<<|=======|>>>>>>>)/m);
    assert.doesNotMatch(contents, /^kind:\s*Secret\s*$/m);
    assert.doesNotMatch(contents, /gh[pousr]_[A-Za-z0-9_]{20,}/);
    assert.doesNotMatch(contents, /sk-[A-Za-z0-9_-]{16,}/);
    assert.doesNotMatch(contents, /LINEAR_API_TOKEN\s*[:=]\s*[^\s#]+/);
  }
});
