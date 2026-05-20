import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { promisify } from 'node:util';
import test from 'node:test';

const execFileAsync = promisify(execFile);

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/minikube/ec2-mirror/kustomization.yaml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('minikube EC2 mirror imports the EC2 stack and renders local-safe patches', async () => {
  const kustomization = await readRepoFile('remote/minikube/ec2-mirror/kustomization.yaml');

  assert.match(kustomization, /\.\.\/\.\.\/argocd\/dd-next-runtime/);
  assert.match(kustomization, /\.\.\/\.\.\/argocd\/messaging/);
  assert.match(kustomization, /\.\.\/\.\.\/argocd\/observability/);
  assert.match(kustomization, /\.\.\/\.\.\/deployments\/gleam-lambda-runner\/k8s\/ec2/);
  assert.match(kustomization, /patches\/repo-hostpath\.yaml/);
  assert.match(kustomization, /patches\/gateway-local-tls\.json/);

  const { stdout: rendered } = await execFileAsync(
    'kubectl',
    ['kustomize', 'remote/minikube/ec2-mirror'],
    {
      cwd: repoRoot,
      maxBuffer: 10 * 1024 * 1024,
    },
  );

  for (const fragment of [
    'name: dd-remote-gateway',
    'name: dd-dev-server-api',
    'name: dd-remote-rest-api',
    'name: dd-agent-worker-broker',
    'name: dd-gleam-lambda-runner',
    'name: dd-gleamlang-server',
    'name: dd-gleam-mcp-server',
    'name: dd-nats',
    'name: dd-grafana',
    'name: dd-prometheus',
  ]) {
    assert(rendered.includes(fragment), `expected rendered mirror to include ${fragment}`);
  }

  assert.match(rendered, /path:\s*\/workspace\/k8s-cluster/);
  assert.match(rendered, /path:\s*\/var\/lib\/dd\/minikube\/bootstrap-workspaces\/dd-dev-server-api/);
  assert.match(rendered, /name:\s*local-gateway-tls/);
  assert.match(rendered, /CLUSTER_DOCTOR_ENABLED:\s*"false"/);
  assert.match(rendered, /WORKER_IMAGE_BUILD_ENABLED:\s*"false"/);
  assert.match(rendered, /name:\s*LAMBDA_IMAGE_BUILD_ENABLED[\s\S]*value:\s*"false"/);
  assert.match(rendered, /name:\s*LAMBDA_PREWARM_CONTAINER_RUNTIMES[\s\S]*value:\s*""/);
  assert.match(rendered, /name:\s*dd-agent-secrets/);
  assert.match(rendered, /name:\s*dd-remote-auth-secrets/);

  assert.doesNotMatch(rendered, /kind:\s*ScaledObject/);
  assert.doesNotMatch(rendered, /\/home\/ec2-user\/codes\/dd\/dd-next-1/);
  assert.doesNotMatch(rendered, /secretName:\s*dd-remote-gateway-tls/);
  assert.doesNotMatch(rendered, /mountPath:\s*\/run\/containerd\/containerd\.sock/);
  assert.doesNotMatch(rendered, /mountPath:\s*\/usr\/local\/bin\/nerdctl/);
});
