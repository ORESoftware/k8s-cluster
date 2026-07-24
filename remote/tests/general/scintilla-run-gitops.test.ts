import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/deployments/scintilla-run-monorepo/README.md'))) {
      return candidate;
    }
  }
  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();
const read = (path: string) => readFile(resolve(repoRoot, path), 'utf8');
const monorepo = 'remote/deployments/scintilla-run-monorepo';

test('k8s dev pins the Scintilla monorepo and bootstraps its app-of-apps release', async () => {
  const [gitmodules, rootApp] = await Promise.all([
    read('.gitmodules'),
    read('remote/argocd/apps/dd-gleam-lambda-runner.application.yaml'),
  ]);
  assert.match(gitmodules, /path = remote\/deployments\/scintilla-run-monorepo/);
  assert.match(gitmodules, /url = git@github\.com:scintilla-run\/scintilla-run-monorepo\.git/);
  assert.match(rootApp, /name: scintilla-run-root/);
  assert.match(rootApp, /targetRevision: main/);
  assert.match(rootApp, /path: gitops\/ec2\/bootstrap/);
  assert.doesNotMatch(rootApp, /remote\/deployments\/gleam-lambda-runner/);
});

test('monorepo pins the full deployable fleet and excludes the Pages site', async () => {
  const modules = await read(`${monorepo}/.gitmodules`);
  for (const app of [
    'gleam-lambda-runner', 'scintilla-app-rs', 'scintilla-backend.rs',
    'scintilla-clients', 'scintilla-interfaces', 'scintilla-mcp-server.rs',
    'scintilla-run-infra', 'scintilla-sync', 'scintilla-ui.dart',
  ]) {
    assert.match(modules, new RegExp(`path = apps/${app.replace('.', '\\.')}`));
  }
  assert.doesNotMatch(modules, /scintilla-run\.github\.io/);
});

test('runner and Argo paths resolve through the nested monorepo shape', async () => {
  const [gleam, child, applications, dockerignore] = await Promise.all([
    read(`${monorepo}/apps/gleam-lambda-runner/gleam.toml`),
    read(`${monorepo}/apps/gleam-lambda-runner/child-runtimes/js-function-runner.mjs`),
    read(`${monorepo}/apps/scintilla-run-infra/k8s/argocd/application-set.yaml`),
    read('remote/.dockerignore'),
  ]);
  assert.match(gleam, /path = "\.\.\/\.\.\/\.\.\/\.\.\/libs\/pg-defs/);
  assert.match(child, /\.\.\/\.\.\/\.\.\/\.\.\/\.\.\/libs\/nats/);
  assert.match(applications, /path: gitops\/ec2\/control-plane/);
  assert.match(applications, /path: gitops\/ec2\/runner/);
  assert.match(
    dockerignore,
    /!deployments\/scintilla-run-monorepo\/apps\/gleam-lambda-runner\/runtime-images\/nodejs\/\*\*/,
  );
  assert.match(
    dockerignore,
    /!deployments\/scintilla-run-monorepo\/apps\/gleam-lambda-runner\/child-runtimes\/js-function-runner\.mjs/,
  );
  assert.match(dockerignore, /!libs\/nats\/subject-defs\/generated\/javascript\/\*\*/);
});

test('the k8s runner manifest pre-pulls and wires every managed runtime', async () => {
  const runner = `${monorepo}/apps/gleam-lambda-runner`;
  const [matrixText, deployment] = await Promise.all([
    read(`${runner}/runtime-images/matrix.json`),
    read(`${runner}/k8s/ec2/dd-gleam-lambda-runner.deployment.yaml`),
  ]);
  const matrix = JSON.parse(matrixText) as {
    runtimes: Array<{ name: string; imageEnvironment: string }>;
  };
  assert.deepEqual(matrix.runtimes.map(({ name }) => name), [
    'nodejs', 'python3', 'ruby', 'bash', 'golang', 'dart',
    'java', 'erlang', 'elixir', 'gleam', 'rust', 'browser',
  ]);
  for (const runtime of matrix.runtimes) {
    assert.match(deployment, new RegExp(`name: cache-runtime-${runtime.name}`));
    assert.match(deployment, new RegExp(`name: ${runtime.imageEnvironment}`));
  }
  assert.match(deployment, /image: ghcr\.io\/scintilla-run\/scintilla-runner:development/);
  assert.match(deployment, /name: LAMBDA_CONTAINER_WORK_TMPFS_SIZE\s+value: 1g/);
});

test('production promotion builds real images, invokes compilers, and renders immutable GitOps', async () => {
  const [workflow, rendererTest] = await Promise.all([
    read(`${monorepo}/.github/workflows/deploy.yml`),
    read(`${monorepo}/apps/scintilla-run-infra/tests/contracts.test.mjs`),
  ]);
  assert.match(workflow, /Prepare the complete k8s build context/);
  assert.match(workflow, /runtime-images-docker\.e2e\.mjs/);
  assert.match(workflow, /Render immutable app-of-apps desired state/);
  assert.match(workflow, /git add gitops\/ec2/);
  assert.match(rendererTest, /execFileSync\('node'.*render-gitops\.mjs/s);
  assert.match(rendererTest, /assert\.doesNotMatch\(runner, \/\:\(\?:development\|dev\|latest\)/);
});
