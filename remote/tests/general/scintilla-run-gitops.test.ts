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
  const [gleam, child, applications] = await Promise.all([
    read(`${monorepo}/apps/gleam-lambda-runner/gleam.toml`),
    read(`${monorepo}/apps/gleam-lambda-runner/child-runtimes/js-function-runner.mjs`),
    read(`${monorepo}/apps/scintilla-run-infra/k8s/argocd/application-set.yaml`),
  ]);
  assert.match(gleam, /path = "\.\.\/\.\.\/\.\.\/\.\.\/libs\/pg-defs/);
  assert.match(child, /\.\.\/\.\.\/\.\.\/\.\.\/\.\.\/libs\/nats/);
  assert.match(applications, /path: gitops\/ec2\/control-plane/);
  assert.match(applications, /path: gitops\/ec2\/runner/);
});
