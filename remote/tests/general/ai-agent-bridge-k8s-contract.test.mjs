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

const deployment = readFileSync(deploymentPath, 'utf8');
const service = readFileSync(servicePath, 'utf8');
const kustomization = readFileSync(kustomizationPath, 'utf8');
const externalSecret = readFileSync(externalSecretPath, 'utf8');
const fixture = readFileSync(fixturePath, 'utf8');
const kindScript = readFileSync(kindScriptPath, 'utf8');

// The one true secret contract for the bridge bearer. Everything that names the
// secret — the deployment, the ExternalSecret that provisions it, the kind smoke
// fixture, and the CI script that seeds it — must agree on BOTH of these, or the
// pod binds a key that was never populated (the exact drift that shipped a broken
// merge: the deployment pointed at dd-agent-secrets/SERVER_AUTH_SECRET while the
// rest of the tree provisioned dd-ai-agent-bridge-secrets/inbox_token).
const SECRET_NAME = 'dd-ai-agent-bridge-secrets';
const SECRET_KEY = 'inbox_token';

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
  // The deployment resolves the binary through bin_name and execs
  // "${CARGO_TARGET_DIR:-target}/release/${bin_name}". Assert the name is the
  // current binary, that the built path is what gets exec'd, and that the
  // retired binary name cannot return.
  assert.match(
    deployment,
    // \b anchors so a substring match on the retired name cannot pass.
    /\bbin_name="fiducia-ai-agent-bridge"/,
    'bin_name must resolve to the current fiducia-ai-agent-bridge binary',
  );
  // Both halves of the contract, kept from the two branches that wrote this test
  // concurrently — they check different things and neither implies the other:
  //   1. the exec path is COMPOSED from bin_name (not a hardcoded literal), and
  //   2. the variable that was existence-checked is what actually becomes PID 1.
  assert.match(
    deployment,
    /\/release\/\$\{bin_name\}/,
    'the built path must be composed from ${bin_name}',
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

test('bridge bearer secret is consistent across deployment, ExternalSecret, fixture, and CI', () => {
  // The ExternalSecret provisions the secret and its key...
  assert.match(
    externalSecret,
    new RegExp(`kind:\\s*ExternalSecret[\\s\\S]*name:\\s*${SECRET_NAME}`),
    `${externalSecretPath} must provision the ${SECRET_NAME} secret`,
  );
  assert.match(
    externalSecret,
    new RegExp(`secretKey:\\s*${SECRET_KEY}`),
    `${externalSecretPath} must populate the ${SECRET_KEY} key`,
  );

  // ...both deployment env bindings consume exactly that secret + key...
  for (const envName of ['API_AUTH_BEARER', 'AI_AGENT_BRIDGE_TOKEN']) {
    const block = envBlock(envName);
    assert.match(block, new RegExp(`name: ${SECRET_NAME}`), `${envName} must reference ${SECRET_NAME}`);
    assert.match(block, new RegExp(`key: ${SECRET_KEY}`), `${envName} must reference key ${SECRET_KEY}`);
  }

  // ...the kind smoke fixture points at the same secret + key...
  assert.match(fixture, new RegExp(`name: ${SECRET_NAME}`));
  assert.match(fixture, new RegExp(`key: ${SECRET_KEY}`));

  // ...and the CI script actually creates that secret with that key, so the smoke
  // exercises the same contract it asserts.
  assert.match(
    kindScript,
    new RegExp(`create secret generic ${SECRET_NAME}`),
    `${kindScriptPath} must create the ${SECRET_NAME} secret`,
  );
  assert.match(kindScript, new RegExp(`--from-literal=${SECRET_KEY}=`));

  // The retired shared cluster credential must not creep back into the running
  // deployment. (The ExternalSecret's comments intentionally name the old
  // dd-agent-secrets/SERVER_AUTH_SECRET to document why it was abandoned, so the
  // negative check is scoped to the manifest that actually wires the bearer.)
  assert.doesNotMatch(deployment, /dd-agent-secrets/, 'deployment must not reuse the shared dd-agent-secrets credential');
  assert.doesNotMatch(deployment, /SERVER_AUTH_SECRET/, 'deployment must not reuse SERVER_AUTH_SECRET');
});

test('deployment keeps the hostPath staleness guard that prevented the 2026-07-31 outage', () => {
  // The crate the mounted hostPath declares must build the exact binary we exec;
  // otherwise fall back to the pinned clone. This turns a silent node/Git drift
  // (crate renamed in Git, node still on the old checkout) into an automatic
  // recovery instead of exec'ing a path that does not exist.
  assert.match(deployment, /bin_name="fiducia-ai-agent-bridge"/);
  assert.match(
    deployment,
    /grep -q "\^name = /,
    'the mounted Cargo.toml must be checked for the target binary before it is trusted',
  );
  assert.match(deployment, /\$\{bin_name\}/);

  // After building, the binary must exist before exec, failing with the real cause
  // rather than bash's bare "No such file or directory".
  // Retained from the main-side contract: the exec'd path must be *derived* from
  // the same checked binary variable, not written out a second time by hand.
  assert.match(
    deployment,
    /built="\$\{CARGO_TARGET_DIR:-target\}\/release\/\$\{bin_name\}"/,
    'the executable path must be derived from the checked binary variable',
  );
  assert.match(deployment, /if \[ ! -x "\$\{built\}" \]; then/);
  assert.match(deployment, /FATAL: cargo did not produce/);
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
