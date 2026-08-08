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
const externalSecretPath =
  'remote/argocd/dd-next-runtime/dd-ai-agent-bridge.externalsecret.yaml';
const fixturePath = 'remote/tests/fixtures/ai-agent-bridge-kind.yaml.tmpl';
const kindScriptPath = 'scripts/ci/test-ai-agent-bridge-kind.sh';

const SOURCE_SHA = 'ec667946b1f8725b6baea8e67ae6a701d602dc04';
const WORKFLOW_RUN = '31264194679';
const IMAGE =
  'ghcr.io/oresoftware/fiducia-ai-agent-bridge@sha256:bbf105c29cdbcec23d87ed0b21cfd548c43982cf6573aaf34a2fb1f4dc69a305';
const SECRET_NAME = 'dd-ai-agent-bridge-secrets';
const SECRET_KEY = 'inbox_token';

const deployment = readFileSync(deploymentPath, 'utf8');
const service = readFileSync(servicePath, 'utf8');
const kustomization = readFileSync(kustomizationPath, 'utf8');
const externalSecret = readFileSync(externalSecretPath, 'utf8');
const fixture = readFileSync(fixturePath, 'utf8');
const kindScript = readFileSync(kindScriptPath, 'utf8');

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

test('bridge runs the exact immutable release image', () => {
  assert.equal(count(deployment, `image: ${IMAGE}`), 1);
  assert.equal(count(deployment, `dd.dev/image-reference: '${IMAGE}'`), 2);
  assert.equal(count(deployment, `dd.dev/source-revision: '${SOURCE_SHA}'`), 2);
  assert.equal(count(deployment, `dd.dev/release-workflow-run: '${WORKFLOW_RUN}'`), 2);
  assert.match(IMAGE, /@sha256:[0-9a-f]{64}$/);
  assert.doesNotMatch(deployment, /image:\s*docker\.io\/library\/rust:/);
  assert.doesNotMatch(deployment, /\bgit clone\b/);
  assert.doesNotMatch(deployment, /\bcargo (?:build|run)\b/);
  assert.doesNotMatch(deployment, /\bGH_PAT\b/);
  assert.doesNotMatch(deployment, /hostPath:/);
  assert.doesNotMatch(deployment, /K8S_GIT_(?:REF|REPOSITORY)/);
  assert.doesNotMatch(deployment, /CARGO_(?:HOME|TARGET_DIR)/);
  assert.doesNotMatch(deployment, /initContainers:/);
  assert.doesNotMatch(deployment, /command:/);
  assert.doesNotMatch(deployment, /args:/);
});

test('non-loopback bridge bind has a required secret-backed bearer', () => {
  assert.match(deployment, /- name: HOST\n\s+value: 0\.0\.0\.0/);
  for (const envName of ['API_AUTH_BEARER', 'AI_AGENT_BRIDGE_TOKEN']) {
    const block = envBlock(envName);
    assert.match(block, /valueFrom:/);
    assert.match(block, /secretKeyRef:/);
    assert.match(block, new RegExp(`name: ${SECRET_NAME}`));
    assert.match(block, new RegExp(`key: ${SECRET_KEY}`));
    assert.doesNotMatch(block, /optional:\s*true/);
    assert.doesNotMatch(block, /\n\s+value:/, 'bearer must not be plaintext');
  }
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

test('distroless pod security and resource ceilings are explicit', () => {
  assert.match(deployment, /automountServiceAccountToken: false/);
  assert.match(deployment, /enableServiceLinks: false/);
  assert.match(deployment, /runAsUser: 65532/);
  assert.match(deployment, /runAsGroup: 65532/);
  assert.match(deployment, /fsGroup: 65532/);
  assert.match(deployment, /allowPrivilegeEscalation: false/);
  assert.match(deployment, /privileged: false/);
  assert.match(deployment, /readOnlyRootFilesystem: true/);
  assert.match(deployment, /runAsNonRoot: true/);
  assert.match(deployment, /capabilities:\n\s+drop:\n\s+- ALL/);
  assert.match(deployment, /seccompProfile:\n\s+type: RuntimeDefault/);
  assert.match(deployment, /resources:\n\s+requests:[\s\S]*?limits:/);
  assert.match(deployment, /emptyDir:\n\s+sizeLimit: 256Mi/);
});

test('applied kustomization registers bridge resources once', () => {
  assert.equal(count(kustomization, '- dd-ai-agent-bridge.deployment.yaml'), 1);
  assert.equal(count(kustomization, '- dd-ai-agent-bridge.service.yaml'), 1);
});

test('bridge bearer secret is consistent across deployment, ExternalSecret, fixture, and CI', () => {
  assert.match(externalSecret, new RegExp(`kind:\\s*ExternalSecret[\\s\\S]*name:\\s*${SECRET_NAME}`));
  assert.match(externalSecret, new RegExp(`secretKey:\\s*${SECRET_KEY}`));
  assert.match(fixture, new RegExp(`name: ${SECRET_NAME}`));
  assert.match(fixture, new RegExp(`key: ${SECRET_KEY}`));
  assert.match(kindScript, new RegExp(`create secret generic ${SECRET_NAME}`));
  assert.match(kindScript, new RegExp(`--from-literal=${SECRET_KEY}=`));
  assert.doesNotMatch(deployment, /dd-agent-secrets/);
  assert.doesNotMatch(deployment, /SERVER_AUTH_SECRET/);
});

test('ephemeral Kubernetes smoke binds the gitlink to a published digest', () => {
  assert.match(kindScript, /source_sha=.*git ls-tree HEAD remote\/deployments\/ai-agent-bridge/);
  assert.match(kindScript, /image_tag="ghcr\.io\/oresoftware\/fiducia-ai-agent-bridge:sha-\$\{source_sha\}"/);
  assert.match(kindScript, /image_digest=.*RepoDigests/);
  assert.match(kindScript, /org\.opencontainers\.image\.revision/);
  assert.match(kindScript, /kind load docker-image/);
  assert.match(kindScript, /__AI_BRIDGE_IMAGE__/);
});

test('bridge exports telemetry to the in-cluster OTEL collector', () => {
  assert.match(envBlock('OTEL_SERVICE_NAME'), /value: dd-ai-agent-bridge/);
  assert.match(
    envBlock('OTEL_EXPORTER_OTLP_ENDPOINT'),
    /value: http:\/\/dd-otel-collector\.observability\.svc\.cluster\.local:4318/,
  );
  assert.match(envBlock('OTEL_EXPORTER_OTLP_PROTOCOL'), /value: http\/protobuf/);
});

const audit = {
  generated_at: new Date().toISOString(),
  deployment: deploymentPath,
  service: servicePath,
  source_sha: SOURCE_SHA,
  release_workflow_run: Number(WORKFLOW_RUN),
  image: IMAGE,
  immutable_runtime: {
    digest_pinned: deployment.includes(`image: ${IMAGE}`),
    no_runtime_builder: !/image:\s*docker\.io\/library\/rust:/.test(deployment),
    no_runtime_git_clone: !deployment.includes('git clone'),
    no_github_pat: !deployment.includes('GH_PAT'),
    no_source_host_path: !deployment.includes('hostPath:'),
    required_api_bearer: !envBlock('API_AUTH_BEARER').includes('optional: true'),
  },
  provider_runner_manifest_present: existsSync(runnerPath),
  known_follow_up: {
    runtime_builder_image: /image:\s*docker\.io\/library\/rust:/.test(deployment),
    runtime_git_clone: deployment.includes('git clone'),
    mutable_git_ref: /K8S_GIT_REF/.test(deployment),
    github_pat_fallback: deployment.includes('name: GH_PAT'),
    source_host_path: deployment.includes('hostPath:'),
    provider_runner_manifest_present: existsSync(runnerPath),
    live_cluster_activation_proven: false,
  },
};

const auditPath = process.env.AI_BRIDGE_AUDIT_PATH;
if (auditPath) {
  writeFileSync(auditPath, `${JSON.stringify(audit, null, 2)}\n`);
}
