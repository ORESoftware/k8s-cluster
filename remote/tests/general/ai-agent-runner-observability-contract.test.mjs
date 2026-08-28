import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';

const runner = fs.readFileSync(
  'remote/argocd/dd-next-runtime/dd-ai-agent-runner.deployment.yaml',
  'utf8',
);
const exporterConfig = fs.readFileSync(
  'remote/argocd/observability/k8s-resource-exporter.configmap.yaml',
  'utf8',
);
const exporterDeployment = fs.readFileSync(
  'remote/argocd/observability/k8s-resource-exporter.deployment.yaml',
  'utf8',
);

test('zero-replica provider runner remains observable before activation', () => {
  assert.match(runner, /name:\s*dd-ai-agent-runner/);
  assert.match(runner, /replicas:\s*0/);
  assert.match(exporterConfig, /DEFAULT_WATCH_APPS[\s\S]*dd-ai-agent-bridge,dd-ai-agent-runner,/);
  assert.match(exporterDeployment, /name:\s*WATCH_APPS[\s\S]*value:[^\n]*dd-ai-agent-runner/);
});

test('runner observability uses the canonical application identity', () => {
  assert.match(runner, /app:\s*dd-ai-agent-runner/);
  assert.doesNotMatch(runner, /app:\s*dd-ai-agent-bridge\s*$/m);
});
