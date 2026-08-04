import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '../../..');
const read = (path) => readFileSync(resolve(root, path), 'utf8');

const cloneNetworkPath =
  'remote/argocd/dd-next-runtime/dd-gha-clone-server.networkpolicy.yaml';
const cloneDeploymentPath =
  'remote/argocd/dd-next-runtime/dd-gha-clone-server.deployment.yaml';
const routerDeploymentPath =
  'remote/argocd/dd-next-runtime/dd-gha-executor-router.deployment.yaml';
const routerConfigPath =
  'remote/argocd/dd-next-runtime/dd-gha-executor-router.configmap.yaml';
const activationDocPath = 'docs/gha-executor-router-activation.md';
const workflowPath = '.github/workflows/gha-clone-server.yml';

function section(text, start, end) {
  const startIndex = text.indexOf(start);
  assert.notEqual(startIndex, -1, `missing section ${start}`);
  const afterStart = text.slice(startIndex + start.length);
  const endIndex = afterStart.indexOf(end);
  return endIndex === -1 ? afterStart : afterStart.slice(0, endIndex);
}

test('clone server accepts only gateway ingress and has no direct build-server egress', () => {
  const policy = read(cloneNetworkPath);
  const ingress = section(policy, '  ingress:\n', '  egress:\n');
  const egress = section(policy, '  egress:\n', '\n---\n');

  assert.match(ingress, /app:\s*dd-remote-gateway/);
  assert.match(ingress, /port:\s*8125/);
  assert.doesNotMatch(ingress, /app:\s*dd-build-server/);
  assert.doesNotMatch(ingress, /port:\s*8100/);

  assert.match(egress, /app:\s*dd-gha-executor-router/);
  assert.match(egress, /port:\s*8126/);
  assert.doesNotMatch(egress, /app:\s*dd-build-server/);
  assert.doesNotMatch(egress, /port:\s*8100/);
});

test('all independent execution gates remain disabled and absent by default', () => {
  const cloneDeployment = read(cloneDeploymentPath);
  const routerDeployment = read(routerDeploymentPath);

  for (const deployment of [cloneDeployment, routerDeployment]) {
    assert.match(deployment, /\breplicas:\s*0\b/);
    assert.match(deployment, /automountServiceAccountToken:\s*false/);
    assert.doesNotMatch(
      deployment,
      /docker\.sock|containerd\.sock|buildkitd\.sock|hostPath:/,
    );
  }
  assert.match(
    cloneDeployment,
    /name:\s*GHA_CLONE_EXECUTION_ENABLED\s+value:\s*"false"/,
  );
  assert.match(
    cloneDeployment,
    /name:\s*GHA_CLONE_WEBHOOK_EXECUTION_ENABLED\s+value:\s*"false"/,
  );
  assert.match(
    routerDeployment,
    /name:\s*GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED\s+value:\s*"false"/,
  );
});

test('disabled Hetzner identity cannot accumulate dormant endpoint or credential state', () => {
  const config = read(routerConfigPath);
  const marker = '"id": "hetzner-secondary"';
  const index = config.indexOf(marker);
  assert.notEqual(index, -1, 'missing disabled Hetzner identity');
  const tail = config.slice(index);

  assert.match(tail, /"provider": "hetzner"/);
  assert.match(tail, /"enabled": false/);
  assert.doesNotMatch(tail, /"url"\s*:/);
  assert.doesNotMatch(tail, /"authPath"\s*:/);
});

test('activation runbook retains pre-submit-only failover and operator gates', () => {
  const doc = read(activationDocPath);
  for (const required of [
    'Actions Runner Controller (ARC)',
    'replicas: zero',
    'Hetzner: declared but disabled',
    'Provider selection may occur only before `POST /builds`',
    'Automatic post-attempt takeover remains blocked',
    'Replace in-pod source compilation with immutable digest-pinned',
    'Prove AWS readiness failure selects Hetzner before submission',
    'Fiducia fencing',
    'DEN-1549 remains',
  ]) {
    assert.ok(doc.includes(required), `activation runbook missing ${required}`);
  }
  assert.doesNotMatch(
    doc,
    /ghp_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|BEGIN (?:RSA |EC )?PRIVATE KEY/,
  );
});

test('continuity workflow watches, runs, and scans the activation controls', () => {
  const workflow = read(workflowPath);
  for (const required of [
    'docs/gha-executor-router-activation.md',
    'general/gha-executor-router-activation.test.mjs',
  ]) {
    assert.ok(workflow.includes(required), `workflow missing ${required}`);
  }
});
