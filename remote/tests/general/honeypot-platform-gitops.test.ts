import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readdir, readFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import test from 'node:test';

const platformPath = 'remote/argocd/clusters/aws/honeypot-platform';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, platformPath, 'kustomization.yaml'))) {
      return candidate;
    }
  }
  throw new Error(`Unable to locate repository root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();
const read = (path: string) => readFile(resolve(repoRoot, path), 'utf8');

async function readTree(path: string): Promise<Array<{ path: string; contents: string }>> {
  const absolute = resolve(repoRoot, path);
  const output: Array<{ path: string; contents: string }> = [];
  for (const entry of await readdir(absolute, { withFileTypes: true })) {
    const relative = join(path, entry.name);
    if (entry.isDirectory()) output.push(...await readTree(relative));
    if (entry.isFile()) output.push({ path: relative, contents: await read(relative) });
  }
  return output;
}

test('honeypot namespace is restricted, bounded, and default-deny', async () => {
  const tenancy = await read(`${platformPath}/tenancy.yaml`);
  assert.match(tenancy, /kind: Namespace\s+metadata:\s+name: honeypot/s);
  for (const mode of ['enforce', 'audit', 'warn']) {
    assert.match(tenancy, new RegExp(`pod-security\\.kubernetes\\.io/${mode}: restricted`));
  }
  assert.match(tenancy, /kind: ResourceQuota/);
  assert.match(tenancy, /pods: "8"/);
  assert.match(tenancy, /persistentvolumeclaims: "0"/);
  assert.match(tenancy, /kind: LimitRange/);
  assert.match(tenancy, /name: honeypot-default-deny/);
  assert.match(tenancy, /policyTypes:\s+- Ingress\s+- Egress/s);
  assert.doesNotMatch(tenancy, /^kind:\s*Secret\s*$/m);
});

test('service identity never receives an automatic Kubernetes token', async () => {
  const serviceAccount = await read(`${platformPath}/serviceaccount.yaml`);
  assert.match(serviceAccount, /kind: ServiceAccount/);
  assert.match(serviceAccount, /name: honeypot\s+namespace: honeypot/s);
  assert.match(serviceAccount, /automountServiceAccountToken: false/);
});

test('AppProject is pinned to one repository and one namespace', async () => {
  const project = await read(`${platformPath}/appproject.yaml`);
  assert.match(project, /name: honeypot\s+namespace: argocd/s);
  assert.match(project, /sourceRepos:\s+- https:\/\/github\.com\/ORESoftware\/honeypot\.rs\.git/s);
  assert.match(project, /destinations:\s+- server: https:\/\/kubernetes\.default\.svc\s+namespace: honeypot/s);
  assert.match(project, /clusterResourceWhitelist: \[\]/);
  assert.match(project, /namespaceResourceBlacklist:[\s\S]*kind: ResourceQuota[\s\S]*kind: LimitRange/);
  assert.doesNotMatch(project, /k8s-cluster\.git/);
  assert.doesNotMatch(project, /namespace:\s*['"]?\*['"]?/);
});

test('platform registration remains inert and contains no workload or public route', async () => {
  const kustomization = await read(`${platformPath}/kustomization.yaml`);
  assert.match(kustomization, /- tenancy\.yaml/);
  assert.match(kustomization, /- serviceaccount\.yaml/);
  assert.match(kustomization, /- appproject\.yaml/);

  const yamlFiles = (await readTree(platformPath)).filter(({ path }) => /\.ya?ml$/.test(path));
  for (const file of yamlFiles) {
    assert.doesNotMatch(
      file.contents,
      /^kind:\s*(?:Application|Deployment|StatefulSet|DaemonSet|Job|CronJob|Service|Ingress|HTTPRoute|Tunnel|ExternalSecret)\s*$/m,
      `${file.path} must remain platform-only and inert`,
    );
  }
});

test('activation guidance forbids DDoS forwarding and retaliation', async () => {
  const readme = await read(`${platformPath}/README.md`);
  assert.match(readme, /Never forward volumetric DDoS traffic to the pod\./);
  assert.match(readme, /Hack-back and retaliatory traffic are prohibited\./);
  assert.match(readme, /immutable signed image/);
  assert.match(readme, /human review/);
});

test('platform files contain no credentials, plaintext Secrets, or merge markers', async () => {
  const files = await readTree(platformPath);
  for (const file of files) {
    assert.doesNotMatch(file.contents, /^(?:<<<<<<<|=======|>>>>>>>)/m);
    assert.doesNotMatch(file.contents, /^kind:\s*Secret\s*$/m);
    assert.doesNotMatch(file.contents, /gh[pousr]_[A-Za-z0-9_]{20,}/);
    assert.doesNotMatch(file.contents, /sk-[A-Za-z0-9_-]{16,}/);
    assert.doesNotMatch(file.contents, /LINEAR_API_TOKEN\s*[:=]\s*[^\s#]+/);
  }
});
