import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/argocd/apps/dd-next-runtime.application.yaml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

function listYamlFiles(directory: string): string[] {
  const entries = readdirSync(directory, { withFileTypes: true });
  const files: string[] = [];

  for (const entry of entries) {
    const absolutePath = join(directory, entry.name);

    if (entry.isDirectory()) {
      files.push(...listYamlFiles(absolutePath));
      continue;
    }

    if (entry.isFile() && /\.ya?ml$/.test(entry.name)) {
      files.push(absolutePath);
    }
  }

  return files;
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

test('argocd external secrets use a cloud-neutral store name', () => {
  const argocdYaml = listYamlFiles(resolve(repoRoot, 'remote/argocd'));

  for (const file of argocdYaml) {
    const contents = readFileSync(file, 'utf8');
    assert.doesNotMatch(contents, /dd-aws-secrets-manager/, file);
  }
});

test('provider secret-store overlays keep the dd-cluster-secrets contract', async () => {
  const providers = ['aws', 'gcp', 'hetzner'] as const;

  for (const provider of providers) {
    const storeFile = `remote/argocd/secrets/providers/${provider}/secret-store.yaml`;
    const store = await readRepoFile(storeFile);

    assert.match(store, /kind:\s*ClusterSecretStore/, provider);
    assert.match(store, /name:\s*dd-cluster-secrets/, provider);
    assert.match(store, new RegExp(`dd\\.dev/cloud-provider:\\s*${provider}`), provider);
  }
});

test('cloud cluster profiles render the same branch with provider overlays', () => {
  const profiles = [
    {
      provider: 'aws',
      providerPath: 'remote/argocd/secrets/providers/aws',
      storageProvisioner: 'ebs.csi.aws.com',
    },
    {
      provider: 'gcp',
      providerPath: 'remote/argocd/secrets/providers/gcp',
      storageProvisioner: 'pd.csi.storage.gke.io',
    },
    {
      provider: 'hetzner',
      providerPath: 'remote/argocd/secrets/providers/hetzner',
      storageProvisioner: 'csi.hetzner.cloud',
    },
  ];

  for (const profile of profiles) {
    const rendered = execFileSync('kubectl', ['kustomize', `remote/argocd/clusters/${profile.provider}`], {
      cwd: repoRoot,
      encoding: 'utf8',
    });

    assert.match(rendered, /name:\s*dd-next-runtime/, profile.provider);
    assert.match(rendered, /name:\s*dd-secret-store/, profile.provider);
    assert.match(rendered, /name:\s*dd-secrets/, profile.provider);
    assert.match(rendered, /repoURL:\s*https:\/\/github\.com\/ORESoftware\/k8s-cluster\.git/, profile.provider);
    assert.doesNotMatch(rendered, /repoURL:\s*git@github\.com:ORESoftware\/k8s-cluster\.git/, profile.provider);
    assert.match(rendered, /targetRevision:\s*dev/, profile.provider);
    assert.match(
      rendered,
      new RegExp(`dd\\.dev/cloud-provider:\\s*${profile.provider}`),
      profile.provider,
    );
    assert.match(
      rendered,
      new RegExp(`path:\\s*${escapeRegExp(profile.providerPath)}`),
      profile.provider,
    );
    assert.match(
      rendered,
      /path:\s*remote\/argocd\/secrets\/common/,
      profile.provider,
    );
    assert.match(
      rendered,
      new RegExp(`provisioner:\\s*${escapeRegExp(profile.storageProvisioner)}`),
      profile.provider,
    );
  }
});

test('hetzner gateway ingress serves the load-balancer sslip host', async () => {
  const ingress = await readRepoFile('remote/hetzner/dd-remote-gateway-ingress.yaml');

  assert.match(ingress, /host:\s*hello\.95-217-171-250\.sslip\.io/);
  assert.match(ingress, /secretName:\s*gateway-public-tls/);
  assert.match(ingress, /name:\s*dd-remote-gateway/);
  assert.match(ingress, /number:\s*443/);
  assert.doesNotMatch(ingress, /hello\.167-233-100-88\.sslip\.io/);
});
