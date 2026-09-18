import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/argocd/dd-next-runtime/kustomization.yaml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

const repoRoot = findRepoRoot();
const removedLocalK8sNames = ['mini' + 'kube', 'mini' + '-kube', 'dev-server' + '-local'];
const removedLocalK8sPattern = new RegExp(
  removedLocalK8sNames.map(escapeRegExp).join('|'),
  'i',
);

function trackedFiles(): string[] {
  const stdout = execFileSync('git', ['ls-files', '-z', '--recurse-submodules'], {
    cwd: repoRoot,
    encoding: 'buffer',
  });

  return stdout.toString('utf8').split('\0').filter(Boolean);
}

test('removed local k8s names stay out of tracked files', () => {
  const offenders: string[] = [];

  for (const relativePath of trackedFiles()) {
    if (removedLocalK8sPattern.test(relativePath)) {
      offenders.push(`${relativePath}:path`);
      continue;
    }

    const contents = readFileSync(resolve(repoRoot, relativePath));
    if (contents.includes(0)) {
      continue;
    }

    const text = contents.toString('utf8');
    const match = removedLocalK8sPattern.exec(text);
    if (match) {
      const line = text.slice(0, match.index).split(/\r?\n/).length;
      offenders.push(`${relativePath}:${line}`);
    }
  }

  assert.deepEqual(offenders, []);
});
