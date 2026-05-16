import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

const repoRoot = resolve(process.cwd(), '..', '..');

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

function parseNumericEnv(yamlText: string, key: string): number {
  const matcher = new RegExp(`name:\\s*${key}[\\s\\S]*?value:\\s*"?(\\d+)"?`, 'm');
  const match = yamlText.match(matcher);
  assert.ok(match, `missing env var ${key}`);
  return Number.parseInt(match[1]!, 10);
}

function yamlScalar(value: string): string {
  return `['"]?${value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}['"]?`;
}

test('ws loadtest manifests configure 5k + 5k clients against websocket endpoint', async () => {
  const rustDeployment = await readRepoFile(
    'remote/ws-loadtest-rs/k8s/ec2/dd-ws-loadtest-rs.deployment.yaml',
  );
  const gleamDeployment = await readRepoFile(
    'remote/gleamlang-ws-loadtest/k8s/ec2/dd-gleamlang-ws-loadtest.deployment.yaml',
  );
  const gleamServerDeployment = await readRepoFile(
    'remote/gleamlang-server/k8s/ec2/dd-gleamlang-server.deployment.yaml',
  );

  const rustCount = parseNumericEnv(rustDeployment, 'CLIENT_COUNT');
  const gleamCount = parseNumericEnv(gleamDeployment, 'CLIENT_COUNT');
  const total = rustCount + gleamCount;

  assert.equal(rustCount, 5_000, 'rust loadtest must open 5k clients');
  assert.equal(gleamCount, 5_000, 'gleam loadtest must open 5k clients');
  assert.equal(total, 10_000, 'combined loadtest target must be 10k clients');

  assert.match(
    rustDeployment,
    /ws:\/\/dd-gleamlang-server\.default\.svc\.cluster\.local:8081\/ws/,
    'rust deployment should target gleam websocket endpoint',
  );
  assert.match(
    gleamDeployment,
    /ws:\/\/dd-gleamlang-server\.default\.svc\.cluster\.local:8081\/ws/,
    'gleam deployment should target gleam websocket endpoint',
  );
  assert.match(
    gleamDeployment,
    /image:\s*ghcr\.io\/gleam-lang\/gleam:v1\.16\.0-node-alpine/,
    'gleam loadtest deployment should use the node-enabled gleam runtime image',
  );
  assert.match(
    rustDeployment,
    /image:\s*docker\.io\/library\/rust:1\.82-bookworm/,
    'rust loadtest deployment should use the full rust toolchain image for cargo run',
  );
  assert.match(
    rustDeployment,
    /\/usr\/local\/cargo\/bin\/cargo run --release/,
    'rust deployment should call cargo from absolute path',
  );
  assert.match(
    gleamServerDeployment,
    /ghcr\.io\/gleam-lang\/gleam:v1\.16\.0-erlang-alpine/,
    'gleam server deployment should use gleam 1.16 erlang runtime',
  );
  assert.match(
    gleamServerDeployment,
    new RegExp(
      `requests:\\s*[\\s\\S]*cpu:\\s*${yamlScalar('1')}[\\s\\S]*memory:\\s*${yamlScalar('1Gi')}`,
    ),
    'gleam server deployment should request 1 CPU and 1Gi memory for startup compilation',
  );
  assert.match(
    gleamServerDeployment,
    new RegExp(
      `limits:\\s*[\\s\\S]*cpu:\\s*${yamlScalar('4')}[\\s\\S]*memory:\\s*${yamlScalar('8Gi')}`,
    ),
    'gleam server deployment should reserve the 8Gi startup compile limit',
  );
});

test('argo applications point to both websocket loadtest deployments', async () => {
  const rustApp = await readRepoFile('remote/argocd/apps/dd-ws-loadtest-rs.application.yaml');
  const gleamApp = await readRepoFile('remote/argocd/apps/dd-ws-loadtest-gleam.application.yaml');

  assert.match(
    rustApp,
    /path:\s*remote\/ws-loadtest-rs\/k8s\/ec2/,
    'rust app should source rust loadtest kustomization',
  );
  assert.match(
    gleamApp,
    /path:\s*remote\/gleamlang-ws-loadtest\/k8s\/ec2/,
    'gleam app should source gleam loadtest kustomization',
  );
});
