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
  const readme = await readRepoFile('remote/deployments/gcs/readme.md');

  assert.match(mongoDeployment, /-\s+--replSet[\s\S]*-\s+rs0/);
  assert.match(mongoDeployment, /rs\.initiate\(\{_id:"rs0"/);
  assert.match(mongoDeployment, /gcs-mongodb\.default\.svc\.cluster\.local:27017/);
  assert.match(readme, /single-node replica set \(`rs0`\)/);
});
