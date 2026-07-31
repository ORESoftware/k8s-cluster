import assert from 'node:assert/strict';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { test } from 'node:test';

const deploymentPath =
  'remote/argocd/dd-next-runtime/dd-ai-agent-bridge.deployment.yaml';
const servicePath =
  'remote/argocd/dd-next-runtime/dd-ai-agent-bridge.service.yaml';
const kustomizationPath = 'remote/argocd/dd-next-runtime/kustomization.yaml';
const runnerPath =
  'remote/argocd/dd-next-runtime/dd-ai-agent-runner.deployment.yaml';

const deployment = readFileSync(deploymentPath, 'utf8');
const service = readFileSync(servicePath, 'utf8');
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

test('bridge deployment executes the current Rust binary', () => {
  assert.match(
    deployment,
    /\bbin_name="fiducia-ai-agent-bridge"/,
    'the deployment must select fiducia-ai-agent-bridge',
  );
  assert.match(
    deployment,
    /exec "\$\{built\}"/,
    'the selected and validated binary must become the container process',
  );
  assert.doesNotMatch(
    deployment,
    /\bbin_name="ai-agent-bridge"/,
    'the retired ai-agent-bridge binary name must not return',
  );
  assert.doesNotMatch(
    deployment,
    /\/release\/ai-agent-bridge(?:["'\s]|$)/,
    'the retired literal ai-agent-bridge executable must not return',
  );
});

test('non-loopback bridge bind has a required secret-backed API bearer', () => {
  assert.match(deployment, /- name: HOST\n\s+value: 0\.0\.0\.0/);
  const block = envBlock('API_AUTH_BEARER');
  assert.match(block, /valueFrom:/);
  assert.match(block, /secretKeyRef:/);
  assert.match(block, /name: dd-ai-agent-bridge-secrets/);
  assert.match(block, /key: inbox_token/);
  assert.doesNotMatch(block, /optional:\s*true/);
  assert.doesNotMatch(block, /\n\s+value:/, 'the bearer must not be plaintext');
});

test('legacy claude inbox cannot become an authentication bypass', () => {
  const block = envBlock('AI_AGENT_BRIDGE_TOKEN');
  assert.match(block, /valueFrom:/);
  assert.match(block, /name: dd-ai-agent-bridge-secrets/);
  assert.match(block, /key: inbox_token/);
  assert.doesNotMatch(block, /optional:\s*true/);
});

test('HTTP, TCP, liveness, and readiness contracts remain aligned', () => {
  assert.match(deployment, /- name: http\n\s+containerPort: 8142/);
  assert.match(deployment, /- name: tcp\n\s+containerPort: 8143/);
  assert.match(deployment, /startupProbe:[\s\S]*?path: \/healthz[\s\S]*?port: http/);
  assert.match(deployment, /readinessProbe:[\s\S]*?path: \/readyz[\s\S]*?port: http/);
  assert.match(deployment, /livenessProbe:[\s\S]*?path: \/healthz[\s\S]*?port: http/);

  assert.match(service, /selector:\n\s+app: dd-ai-agent-bridge/);
  assert.match(service, /- name: http\n\s+port: 8142\n\s+targetPort: http/);
  assert.match(service, /- name: tcp\n\s+port: 8143\n\s+targetPort: tcp/);
  assert.doesNotMatch(service, /type:\s*(?:NodePort|LoadBalancer)/);
});

test('pod security and resource ceilings are explicit', () => {
  assert.match(deployment, /automountServiceAccountToken: false/);
  assert.match(deployment, /allowPrivilegeEscalation: false/);
  assert.match(deployment, /readOnlyRootFilesystem: true/);
  assert.match(deployment, /runAsNonRoot: true/);
  assert.match(deployment, /capabilities:\n\s+drop:\n\s+- ALL/);
  assert.match(deployment, /seccompProfile:\n\s+type: RuntimeDefault/);
  assert.match(deployment, /resources:\n\s+requests:[\s\S]*?limits:/);
});

test('the applied runtime kustomization registers bridge deployment and service once', () => {
  assert.equal(count(kustomization, '- dd-ai-agent-bridge.deployment.yaml'), 1);
  assert.equal(count(kustomization, '- dd-ai-agent-bridge.service.yaml'), 1);
});

const audit = {
  generated_at: new Date().toISOString(),
  deployment: deploymentPath,
  service: servicePath,
  startup_contract: {
    current_binary: deployment.includes('bin_name="fiducia-ai-agent-bridge"'),
    required_api_bearer: !envBlock('API_AUTH_BEARER').includes('optional: true'),
    healthz: deployment.includes('path: /healthz'),
    readyz: deployment.includes('path: /readyz'),
    http_port: service.includes('port: 8142'),
    tcp_port: service.includes('port: 8143'),
  },
  known_follow_up: {
    runtime_builder_image: /image:\s*docker\.io\/library\/rust:/.test(deployment),
    runtime_git_clone: deployment.includes('git clone'),
    mutable_git_ref: deployment.includes('K8S_GIT_REF') && deployment.includes('value: dev'),
    github_pat_fallback: deployment.includes('name: GH_PAT'),
    source_host_path: deployment.includes('hostPath:'),
    provider_runner_manifest_present: existsSync(runnerPath),
  },
};

const auditPath = process.env.AI_BRIDGE_AUDIT_PATH;
if (auditPath) {
  writeFileSync(auditPath, `${JSON.stringify(audit, null, 2)}\n`);
}
