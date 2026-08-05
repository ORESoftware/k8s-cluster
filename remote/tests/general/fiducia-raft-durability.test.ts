import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/argocd/fiducia/kustomization.yaml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

function renderFiducia(): string {
  return execFileSync('kubectl', ['kustomize', 'remote/argocd/fiducia'], {
    cwd: repoRoot,
    encoding: 'utf8',
  });
}

function fiduciaNodeDocument(rendered: string): string {
  const document = rendered
    .split(/^---\s*$/m)
    .find(
      (candidate) =>
        /kind:\s*StatefulSet\b/.test(candidate) &&
        /name:\s*fiducia-node\b/.test(candidate),
    );

  assert.ok(document, 'rendered Fiducia node StatefulSet must exist');
  return document;
}

test('authoritative Fiducia Raft data renders as a retained gp3 PVC', () => {
  const node = fiduciaNodeDocument(renderFiducia());

  assert.match(node, /persistentVolumeClaimRetentionPolicy:\s*\n\s*whenDeleted:\s*Retain\s*\n\s*whenScaled:\s*Retain/);
  assert.match(node, /volumeClaimTemplates:/);
  assert.match(
    node,
    /volumeClaimTemplates:[\s\S]*name:\s*data[\s\S]*accessModes:[\s\S]*ReadWriteOnce/,
  );
  assert.match(node, /storageClassName:\s*gp3/);
  assert.match(node, /storage:\s*20Gi/);
  assert.match(node, /dd\.dev\/data-class:\s*authoritative-raft/);
  assert.match(node, /dd\.dev\/backup-required:\s*["']?true["']?/);
  assert.match(node, /mountPath:\s*\/var\/lib\/fiducia/);
});

test('rendered authoritative Raft volume cannot regress to emptyDir', () => {
  const node = fiduciaNodeDocument(renderFiducia());
  const podVolumes = node.match(/\n\s{6}volumes:\n[\s\S]*?(?=\n\s{2}volumeClaimTemplates:|$)/)?.[0] ?? '';

  assert.doesNotMatch(podVolumes, /- name:\s*data\s*\n\s*emptyDir:/);
  assert.match(podVolumes, /- name:\s*tmp\s*\n\s*emptyDir:/);
});

test('durability patch records one-voter-at-a-time rollout and deletion controls', async () => {
  const patch = await readFile(
    resolve(repoRoot, 'remote/argocd/fiducia/fiducia-node.durable-storage.patch.yaml'),
    'utf8',
  );

  assert.match(patch, /rollingUpdate:[\s\S]*partition:\s*0/);
  assert.match(patch, /whenDeleted:\s*Retain/);
  assert.match(patch, /whenScaled:\s*Retain/);
  assert.match(patch, /dd\.dev\/deletion-approval:\s*["']security-and-platform["']/);
  assert.match(patch, /\$patch:\s*delete/);
});

test('durability runbook keeps backup and clean-room restore as production gates', async () => {
  const runbook = await readFile(
    resolve(repoRoot, 'docs/fiducia-raft-durability-runbook.md'),
    'utf8',
  );

  assert.match(runbook, /one voting member at a time/i);
  assert.match(runbook, /clean-room restore/i);
  assert.match(runbook, /encrypted/i);
  assert.match(runbook, /key ID/i);
  assert.match(runbook, /do not delete/i);
  assert.match(runbook, /production gate/i);
});
