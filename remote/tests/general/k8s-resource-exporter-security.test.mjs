import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const manifestPath = new URL(
  '../../argocd/observability/k8s-resource-exporter.deployment.yaml',
  import.meta.url,
);

async function exporterContainer(): Promise<string> {
  const manifest = await readFile(manifestPath, 'utf8');
  const start = manifest.indexOf('        - name: exporter\n');
  assert.notEqual(start, -1, 'resource exporter container is missing');
  const end = manifest.indexOf('\n      volumes:\n', start);
  assert.notEqual(end, -1, 'resource exporter container boundary is missing');
  return manifest.slice(start, end);
}

test('resource exporter uses a restricted non-root container profile', async () => {
  const container = await exporterContainer();

  assert.match(container, /allowPrivilegeEscalation:\s*false/);
  assert.match(container, /capabilities:\s*\n\s*drop:\s*\n\s*- ALL/);
  assert.match(container, /readOnlyRootFilesystem:\s*true/);
  assert.match(container, /runAsGroup:\s*65532/);
  assert.match(container, /runAsNonRoot:\s*true/);
  assert.match(container, /runAsUser:\s*65532/);
  assert.match(container, /seccompProfile:\s*\n\s*type:\s*RuntimeDefault/);
  assert.doesNotMatch(container, /runAs(?:User|Group):\s*0(?:\s|$)/);
});

test('read-only Python runtime does not attempt bytecode writes', async () => {
  const container = await exporterContainer();

  assert.match(
    container,
    /- name:\s*PYTHONDONTWRITEBYTECODE\s*\n\s*value:\s*['"]?1['"]?/,
  );
  assert.match(container, /mountPath:\s*\/app\s*\n\s*readOnly:\s*true/);
});
