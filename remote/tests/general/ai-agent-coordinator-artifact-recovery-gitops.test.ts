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
const revision = process.env.ARTIFACT_RECOVERY_REVISION ?? 'c5a08d1afd26d2982e53fc2066bde9cf64ee5d19';
const appPath = 'remote/argocd/clusters/aws/ai-agent-coordinator-artifact-recovery.application.yaml';
const awsKustomization = 'remote/argocd/clusters/aws/kustomization.yaml';

test('AWS registers one exact, manually synchronized artifact-recovery Application', async () => {
  const application = await read(appPath);
  assert.match(application, /kind: Application/);
  assert.match(application, /name: ai-agent-coordinator-artifact-recovery\s+namespace: argocd/s);
  assert.match(application, /denman\.linear\.app\/issue: DEN-3179/);
  assert.match(application, /oresoftware\.dev\/activation: disabled/);
  assert.match(application, /project: ai-agent-coordinator/);
  assert.match(application, /repoURL: https:\/\/github\.com\/ORESoftware\/ai-agent-coordinator\.rs\.git/);
  assert.match(application, new RegExp(`targetRevision: ${revision}`));
  assert.match(application, /path: deploy\/continuous-artifact-recovery\/k8s/);
  assert.match(application, /server: https:\/\/kubernetes\.default\.svc\s+namespace: ai-agent-coordinator/s);
  assert.match(application, /ServerSideApply=true/);
  assert.match(application, /PruneLast=true/);
  assert.doesNotMatch(application, /targetRevision: (?:main|master|dev|HEAD)/);
  assert.doesNotMatch(application, /^\s*automated:/m);
  assert.doesNotMatch(application, /CreateNamespace=true/);
});

test('the recovery Application is registered once and only in the AWS cluster overlay', async () => {
  const aws = await read(awsKustomization);
  assert.equal((aws.match(/- ai-agent-coordinator-artifact-recovery\.application\.yaml/g) ?? []).length, 1);
  for (const cloud of ['gcp', 'hetzner']) {
    const contents = await read(`remote/argocd/clusters/${cloud}/kustomization.yaml`);
    assert.doesNotMatch(contents, /ai-agent-coordinator-artifact-recovery/);
  }
});

test('registration source contains no credentials, plaintext Secret, or conflict markers', async () => {
  const files = await Promise.all([appPath, awsKustomization].map(read));
  for (const contents of files) {
    assert.doesNotMatch(contents, /^(?:<<<<<<<|=======|>>>>>>>)/m);
    assert.doesNotMatch(contents, /^kind:\s*Secret\s*$/m);
    assert.doesNotMatch(contents, /gh[pousr]_[A-Za-z0-9_]{20,}/);
    assert.doesNotMatch(contents, /lin_api_[A-Za-z0-9]{20,}/);
    assert.doesNotMatch(contents, /sk-[A-Za-z0-9_-]{16,}/);
    assert.doesNotMatch(contents, /-----BEGIN [A-Z ]*PRIVATE KEY-----/);
  }
});
