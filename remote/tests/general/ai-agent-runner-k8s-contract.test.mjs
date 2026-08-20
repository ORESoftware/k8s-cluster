import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';
import { test } from 'node:test';

const deploymentPath =
  'remote/argocd/dd-next-runtime/dd-ai-agent-runner.deployment.yaml';
const networkPolicyPath =
  'remote/argocd/dd-next-runtime/dd-ai-agent-runner.networkpolicy.yaml';
const kustomizationPath = 'remote/argocd/dd-next-runtime/kustomization.yaml';

const SOURCE_SHA = 'd4ade93a01f79b0347e36c48e6d26b236f83f011';
const WORKFLOW_RUN = '31278723389';
const IMAGE =
  'ghcr.io/oresoftware/fiducia-ai-agent-runner@sha256:940ce31bd425081efc925fafe9065ab77e301addb3b916eba620c156658c7161';

const deployment = readFileSync(deploymentPath, 'utf8');
const networkPolicy = readFileSync(networkPolicyPath, 'utf8');
const kustomization = readFileSync(kustomizationPath, 'utf8');

function count(text, needle) {
  return text.split(needle).length - 1;
}

function envBlock(name) {
  const marker = `            - name: ${name}\n`;
  const start = deployment.indexOf(marker);
  assert.notEqual(start, -1, `${name} is missing from ${deploymentPath}`);
  const next = deployment.indexOf('\n            - name: ', start + marker.length);
  return deployment.slice(start, next === -1 ? deployment.length : next);
}

test('runner is held at zero replicas on one exact reviewed release', () => {
  assert.match(deployment, /^\s*replicas:\s*0\s*$/m);
  assert.equal(count(deployment, `image: ${IMAGE}`), 1);
  assert.equal(count(deployment, `dd.dev/image-reference: '${IMAGE}'`), 2);
  assert.equal(count(deployment, `dd.dev/source-revision: '${SOURCE_SHA}'`), 2);
  assert.equal(count(deployment, `dd.dev/release-workflow-run: '${WORKFLOW_RUN}'`), 2);
  assert.match(deployment, /dd\.dev\/activation-mode: held-zero/);
  assert.match(IMAGE, /@sha256:[0-9a-f]{64}$/);
});

test('runner contains no source checkout, compiler, PAT, or mutable runtime command', () => {
  assert.doesNotMatch(deployment, /\bgit clone\b/);
  assert.doesNotMatch(deployment, /\bcargo (?:build|run)\b/);
  assert.doesNotMatch(deployment, /\bGH_PAT\b/);
  assert.doesNotMatch(deployment, /hostPath:/);
  assert.doesNotMatch(deployment, /initContainers:/);
  assert.doesNotMatch(deployment, /^\s+command:/m);
  assert.doesNotMatch(deployment, /^\s+args:/m);
});

test('runner pod security and resource ceilings are explicit', () => {
  assert.match(deployment, /automountServiceAccountToken: false/);
  assert.match(deployment, /enableServiceLinks: false/);
  assert.match(deployment, /runAsNonRoot: true/);
  assert.match(deployment, /runAsUser: 65532/);
  assert.match(deployment, /runAsGroup: 65532/);
  assert.match(deployment, /fsGroup: 65532/);
  assert.match(deployment, /allowPrivilegeEscalation: false/);
  assert.match(deployment, /readOnlyRootFilesystem: true/);
  assert.match(deployment, /capabilities:\n\s+drop:\n\s+- ALL/);
  assert.match(deployment, /seccompProfile:\n\s+type: RuntimeDefault/);
  assert.match(deployment, /resources:\n\s+requests:[\s\S]*?limits:/);
  assert.match(deployment, /emptyDir:\n\s+sizeLimit: 64Mi/);
});

test('provider secrets are required and the bridge bearer remains secret-backed', () => {
  assert.match(
    deployment,
    /envFrom:\n\s+- secretRef:\n\s+name: dd-ai-agent-runner-secrets/,
  );
  assert.doesNotMatch(
    deployment,
    /name: dd-ai-agent-runner-secrets[\s\S]{0,80}optional:\s*true/,
  );
  const bearer = envBlock('AI_AGENT_RUNNER_BRIDGE_BEARER');
  assert.match(bearer, /valueFrom:/);
  assert.match(bearer, /name: dd-ai-agent-bridge-secrets/);
  assert.match(bearer, /key: inbox_token/);
  assert.doesNotMatch(bearer, /\n\s+value:/);
});

test('runner health contract is isolated on port 8144', () => {
  assert.match(envBlock('AI_AGENT_RUNNER_HEALTH_HOST'), /value: 0\.0\.0\.0/);
  assert.match(envBlock('AI_AGENT_RUNNER_HEALTH_PORT'), /value: '8144'/);
  assert.match(deployment, /- name: health\n\s+containerPort: 8144/);
  assert.match(deployment, /readinessProbe:[\s\S]*?path: \/readyz[\s\S]*?port: health/);
  assert.match(deployment, /livenessProbe:[\s\S]*?path: \/healthz[\s\S]*?port: health/);
});

test('runner NetworkPolicy permits only required health and egress paths', () => {
  assert.match(networkPolicy, /policyTypes:\n\s+- Ingress\n\s+- Egress/);
  assert.match(networkPolicy, /kubernetes\.io\/metadata\.name: observability/);
  assert.match(networkPolicy, /app: dd-prometheus/);
  assert.match(networkPolicy, /port: 8144/);
  assert.match(networkPolicy, /kubernetes\.io\/metadata\.name: kube-system/);
  assert.match(networkPolicy, /port: 53/);
  assert.match(networkPolicy, /app: dd-ai-agent-bridge/);
  assert.match(networkPolicy, /port: 8142/);
  assert.match(networkPolicy, /cidr: 0\.0\.0\.0\/0/);
  assert.match(networkPolicy, /port: 443/);
  for (const cidr of [
    '10.0.0.0/8',
    '127.0.0.0/8',
    '169.254.0.0/16',
    '172.16.0.0/12',
    '192.168.0.0/16',
  ]) {
    assert.ok(networkPolicy.includes(`- ${cidr}`), `${cidr} must remain excluded`);
  }
});

test('applied overlay registers runner resources exactly once', () => {
  assert.equal(count(kustomization, '- dd-ai-agent-runner.deployment.yaml'), 1);
  assert.equal(count(kustomization, '- dd-ai-agent-runner.networkpolicy.yaml'), 1);
});

const audit = {
  generated_at: new Date().toISOString(),
  source_sha: SOURCE_SHA,
  release_workflow_run: Number(WORKFLOW_RUN),
  image: IMAGE,
  replicas_zero: /^\s*replicas:\s*0\s*$/m.test(deployment),
  immutable_runtime: !/(?:git clone|cargo build|GH_PAT|hostPath:)/.test(deployment),
  required_provider_secret: !/name: dd-ai-agent-runner-secrets[\s\S]{0,80}optional:\s*true/.test(
    deployment,
  ),
};

const auditPath = process.env.AI_AGENT_RUNNER_AUDIT_PATH;
if (auditPath) {
  writeFileSync(auditPath, `${JSON.stringify(audit, null, 2)}\n`);
}
