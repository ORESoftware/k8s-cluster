import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

const repoRoot = resolve(new URL('../../..', import.meta.url).pathname);

async function readRepoFile(path: string): Promise<string> {
  return readFile(resolve(repoRoot, path), 'utf8');
}

test('gcs mongodb runs as a single-node replica set for chat transactions', async () => {
  const mongoDeployment = await readRepoFile(
    'remote/deployments/gcs/k8s/ec2/gcs-mongodb.deployment.yaml',
  );
  const headlessService = await readRepoFile(
    'remote/deployments/gcs/k8s/ec2/gcs-mongodb-headless.service.yaml',
  );
  const kustomization = await readRepoFile('remote/deployments/gcs/k8s/ec2/kustomization.yaml');
  const readme = await readRepoFile('remote/deployments/gcs/readme.md');

  assert.match(mongoDeployment, /kind:\s*StatefulSet/);
  assert.match(mongoDeployment, /serviceName:\s*gcs-mongodb-headless/);
  assert.match(mongoDeployment, /-\s+--replSet[\s\S]*-\s+rs0/);
  assert.match(mongoDeployment, /rs\.initiate\(\{_id:"rs0"/);
  assert.match(mongoDeployment, /gcs-mongodb-0\.gcs-mongodb-headless\.default\.svc\.cluster\.local:27017/);
  assert.match(headlessService, /clusterIP:\s*None/);
  assert.match(headlessService, /publishNotReadyAddresses:\s*true/);
  assert.match(kustomization, /gcs-mongodb-headless\.service\.yaml/);
  assert.match(readme, /single-node StatefulSet replica set\s*\(`rs0`\)/);
});
