import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'AGENTS.md'))) return candidate;
  }
  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('edge cache warmer remains fail-closed until pilot approval', async () => {
  const kustomization = await readRepoFile(
    'remote/argocd/edge-cache-warmer/kustomization.yaml',
  );
  const cron = await readRepoFile('remote/argocd/edge-cache-warmer/cronjob.yaml');
  const targets = await readRepoFile(
    'remote/argocd/edge-cache-warmer/targets.configmap.yaml',
  );
  const networkPolicy = await readRepoFile(
    'remote/argocd/edge-cache-warmer/networkpolicy.yaml',
  );
  const application = await readRepoFile(
    'remote/argocd/apps/edge-cache-warmer.application.yaml',
  );
  const runbook = await readRepoFile('remote/argocd/edge-cache-warmer/readme.md');

  assert.match(kustomization, /cronjob\.yaml/);
  assert.match(kustomization, /networkpolicy\.yaml/);
  assert.match(cron, /kind:\s*CronJob/);
  assert.match(cron, /namespace:\s*edge-cache-warmer/);
  assert.match(cron, /suspend:\s*true/);
  assert.match(cron, /concurrencyPolicy:\s*Forbid/);
  assert.match(cron, /automountServiceAccountToken:\s*false/);
  assert.match(cron, /runAsNonRoot:\s*true/);
  assert.match(cron, /readOnlyRootFilesystem:\s*true/);
  assert.match(cron, /EDGE_CACHE_WARMER_GLOBAL_PAUSE[\s\S]*value:\s*["']true["']/);
  assert.match(cron, /bootstrap_suspended/);
  assert.doesNotMatch(cron, /secretKeyRef:/);
  assert.match(targets, /global_pause:\s*true/);
  assert.match(targets, /deny_first_labels:[\s\S]*- api[\s\S]*- app[\s\S]*- www/);
  assert.match(targets, /domains:\s*\[\]/);
  assert.match(networkPolicy, /ingress:\s*\[\]/);
  assert.match(networkPolicy, /cidr:\s*0\.0\.0\.0\/0/);
  assert.match(networkPolicy, /- 10\.0\.0\.0\/8/);
  assert.match(networkPolicy, /- 169\.254\.0\.0\/16/);
  assert.match(networkPolicy, /- 192\.168\.0\.0\/16/);
  assert.match(application, /targetRevision:\s*dev/);
  assert.match(application, /path:\s*remote\/argocd\/edge-cache-warmer/);
  assert.match(runbook, /intentionally inert/);
  assert.match(runbook, /A partial activation must fail closed/);
});
