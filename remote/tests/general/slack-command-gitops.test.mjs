import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { test } from 'node:test';

const bundlePath =
  'remote/argocd/dd-next-runtime/dd-ai-agent-bridge.service.yaml';
const kustomizationPath = 'remote/argocd/dd-next-runtime/kustomization.yaml';
const workflowPath = '.github/workflows/slack-command-gitops.yml';
const sourceRoot = process.env.SLACK_COMMAND_SOURCE_ROOT;
assert.ok(sourceRoot, 'SLACK_COMMAND_SOURCE_ROOT must select the exact reviewed source');

const SOURCE_SHA = '01abb601a3b6a6cfa917094daf17cb9fe1c54f21';
const WORKFLOW_RUN = '31215966809';
const IMAGE =
  'ghcr.io/oresoftware/fiducia-slack-command@sha256:cba2bf92408589df478ebfb19ce3db01d7eafbbf05e2a0ead3afb843a690b72d';

const bundle = readFileSync(bundlePath, 'utf8');
const kustomization = readFileSync(kustomizationPath, 'utf8');
const workflow = readFileSync(workflowPath, 'utf8');
const registryPath = path.join(sourceRoot, 'config/alex-main-agent.channels.json');
const manifestPath = path.join(sourceRoot, 'slack-app/manifest.yaml');
const registry = JSON.parse(readFileSync(registryPath, 'utf8'));
const manifest = readFileSync(manifestPath, 'utf8');

const documents = bundle
  .split(/^---\s*$/m)
  .map((document) => document.trim())
  .filter(Boolean);

function identity(document) {
  const kind = document.match(/^kind:\s*(\S+)\s*$/m)?.[1];
  const metadata = document.match(/^metadata:\n((?:  .*\n?)*)/m)?.[1] ?? '';
  const name = metadata.match(/^  name:\s*(\S+)\s*$/m)?.[1];
  return kind && name ? `${kind}/${name}` : undefined;
}

function resource(kind, name) {
  const document = documents.find((candidate) => identity(candidate) === `${kind}/${name}`);
  assert.ok(document, `${kind}/${name} is missing from ${bundlePath}`);
  return document;
}

function envBlock(deployment, name) {
  const marker = `            - name: ${name}\n`;
  const start = deployment.indexOf(marker);
  assert.notEqual(start, -1, `${name} is missing from the Slack deployment`);
  const next = deployment.indexOf('\n            - name: ', start + marker.length);
  return deployment.slice(start, next === -1 ? deployment.length : next);
}

const bridgeService = resource('Service', 'dd-ai-agent-bridge');
const externalSecret = resource('ExternalSecret', 'dd-slack-command-secrets');
const pvc = resource('PersistentVolumeClaim', 'dd-slack-command-state');
const deployment = resource('Deployment', 'dd-slack-command');
const service = resource('Service', 'dd-slack-command');
const networkPolicy = resource('NetworkPolicy', 'dd-slack-command');
const pdb = resource('PodDisruptionBudget', 'dd-slack-command');
const ingress = resource('Ingress', 'dd-slack-command');

test('bundle contains one exact Slack command resource set and preserves the bridge service', () => {
  assert.deepEqual(
    documents.map(identity).filter(Boolean).sort(),
    [
      'Deployment/dd-slack-command',
      'ExternalSecret/dd-slack-command-secrets',
      'Ingress/dd-slack-command',
      'NetworkPolicy/dd-slack-command',
      'PersistentVolumeClaim/dd-slack-command-state',
      'PodDisruptionBudget/dd-slack-command',
      'Service/dd-ai-agent-bridge',
      'Service/dd-slack-command',
    ].sort(),
  );
  assert.match(bridgeService, /- name: http\n\s+port: 8142\n\s+targetPort: http/);
  assert.match(bridgeService, /- name: tcp\n\s+port: 8143\n\s+targetPort: tcp/);
  assert.equal(
    kustomization.split('- dd-ai-agent-bridge.service.yaml').length - 1,
    1,
  );
});

test('Slack command runs the exact immutable release without source or compiler material', () => {
  assert.equal(deployment.split(`image: ${IMAGE}`).length - 1, 1);
  assert.equal(deployment.split(`dd.dev/image-reference: '${IMAGE}'`).length - 1, 2);
  assert.equal(deployment.split(`dd.dev/source-revision: '${SOURCE_SHA}'`).length - 1, 2);
  assert.equal(deployment.split(`dd.dev/release-workflow-run: '${WORKFLOW_RUN}'`).length - 1, 2);
  assert.match(IMAGE, /@sha256:[0-9a-f]{64}$/);
  assert.doesNotMatch(deployment, /image:\s*docker\.io\/library\/rust:/);
  assert.doesNotMatch(deployment, /\bgit clone\b/);
  assert.doesNotMatch(deployment, /\bcargo (?:build|run)\b/);
  assert.doesNotMatch(deployment, /\bGH_PAT\b/);
  assert.doesNotMatch(deployment, /K8S_GIT_(?:REF|REPOSITORY)/);
  assert.doesNotMatch(deployment, /CARGO_(?:HOME|TARGET_DIR)/);
  assert.doesNotMatch(deployment, /source-bootstrap|source-cache|target-cache/);
  assert.doesNotMatch(deployment, /initContainers:/);
  assert.doesNotMatch(deployment, /command:/);
  assert.doesNotMatch(deployment, /args:/);
  assert.match(
    envBlock(deployment, 'SLACK_PROJECT_REGISTRY_PATH'),
    /value: \/etc\/alex-main-agent\/alex-main-agent\.channels\.json/,
  );
});

test('dispatch remains signed, identity-bound, stateful, and dry-run', () => {
  assert.match(envBlock(deployment, 'SLACK_COMMAND_DRY_RUN'), /value: "true"/);
  assert.match(envBlock(deployment, 'SLACK_EXPECTED_APP_ID'), /value: A0BMBAMM5NJ/);
  assert.match(envBlock(deployment, 'SLACK_EXPECTED_TEAM_ID'), /value: T01B3C83PMK/);
  assert.match(envBlock(deployment, 'SLACK_COMMAND_STATE_DIR'), /value: \/var\/lib\/slack-command\/runs/);
  assert.match(pvc, /storageClassName: dd-block/);
  assert.match(pvc, /ReadWriteOnce/);
  assert.match(deployment, /claimName: dd-slack-command-state/);
  for (const name of [
    'SLACK_BOT_TOKEN',
    'SLACK_SIGNING_SECRET',
    'SLACK_BRIDGE_BEARER',
    'SLACK_COORDINATOR_BEARER',
  ]) {
    const block = envBlock(deployment, name);
    assert.match(block, /valueFrom:/);
    assert.match(block, /name: dd-slack-command-secrets/);
    assert.doesNotMatch(block, /\n\s+value:/);
  }
});

test('ExternalSecret binds the four narrowly named runtime values', () => {
  for (const key of [
    'SLACK_BOT_TOKEN',
    'SLACK_SIGNING_SECRET',
    'SLACK_BRIDGE_BEARER',
    'SLACK_COORDINATOR_BEARER',
  ]) {
    assert.equal(externalSecret.split(`secretKey: ${key}`).length - 1, 1);
  }
  assert.match(externalSecret, /creationPolicy: Owner/);
  assert.match(externalSecret, /name: dd-cluster-secrets/);
  assert.doesNotMatch(externalSecret, /xox[baprs]-|sk-[A-Za-z0-9]/);
});

test('public ingress exposes only three exact Slack-signed POST routes', () => {
  const paths = [...ingress.matchAll(/^\s*path:\s*(\S+)\s*$/gm)].map((match) => match[1]);
  assert.deepEqual(paths, [
    '/slack/commands/ores-claude',
    '/slack/commands/ores-chatgpt',
    '/slack/interactions',
  ]);
  assert.equal((ingress.match(/pathType: Exact/g) ?? []).length, 3);
  assert.match(ingress, /host: api\.fiducia\.cloud/);
  assert.match(service, /type: ClusterIP/);
  assert.match(service, /port: 8151/);
});

test('pod and network boundaries remain explicit', () => {
  assert.match(deployment, /automountServiceAccountToken: false/);
  assert.match(deployment, /enableServiceLinks: false/);
  assert.match(deployment, /runAsUser: 65532/);
  assert.match(deployment, /runAsGroup: 65532/);
  assert.match(deployment, /fsGroup: 65532/);
  assert.match(deployment, /allowPrivilegeEscalation: false/);
  assert.match(deployment, /privileged: false/);
  assert.match(deployment, /readOnlyRootFilesystem: true/);
  assert.match(deployment, /capabilities:\n\s+drop:\n\s+- ALL/);
  assert.match(deployment, /seccompProfile:\n\s+type: RuntimeDefault/);
  assert.match(deployment, /resources:\n\s+requests:[\s\S]*?limits:/);
  assert.match(pdb, /minAvailable: 1/);
  assert.match(networkPolicy, /policyTypes:\n\s+- Ingress\n\s+- Egress/);
  assert.match(networkPolicy, /kubernetes\.io\/metadata\.name: ingress-nginx/);
  assert.match(networkPolicy, /port: 8142/);
  assert.match(networkPolicy, /kubernetes\.io\/metadata\.name: ai-agent-coordinator/);
  assert.match(networkPolicy, /port: 4318/);
  assert.match(networkPolicy, /port: 443/);
});

test('reviewed source retains thirteen bounded channel bindings', () => {
  assert.equal(registry.schema_version, 1);
  assert.equal(registry.bindings.length, 13);
  const channels = new Set();
  for (const binding of registry.bindings) {
    assert.equal(binding.workspace_id, 'T01B3C83PMK');
    assert.equal(binding.linear_team_key, 'DEN');
    assert.equal(binding.write_policy, 'draft_pull_request');
    assert.ok(binding.default_repository);
    assert.ok(binding.repository_allowlist.includes(binding.default_repository));
    assert.ok(binding.allowed_agent_modes.includes('claude'));
    assert.ok(binding.allowed_agent_modes.includes('chatgpt'));
    assert.ok(binding.allowed_agent_modes.includes('both-parallel'));
    assert.ok(binding.budget_policy.max_concurrent_runs > 0);
    assert.ok(binding.budget_policy.max_runtime_secs > 0);
    assert.ok(binding.budget_policy.max_tokens > 0);
    assert.ok(!channels.has(binding.channel_id), `duplicate channel ${binding.channel_id}`);
    channels.add(binding.channel_id);
  }
});

test('reviewed Slack manifest maps canonical commands and aliases to exact ingress routes', () => {
  for (const command of [
    '/ores-claude',
    '/ores-chatgpt',
    '/x-claude',
    '/x-chatgpt',
    '/my-claude',
    '/my-chatgpt',
  ]) {
    assert.match(manifest, new RegExp(`command: ${command.replace('/', '\\/')}`));
  }
  assert.equal(
    (manifest.match(/url: https:\/\/api\.fiducia\.cloud\/slack\/commands\/ores-claude/g) ?? []).length,
    3,
  );
  assert.equal(
    (manifest.match(/url: https:\/\/api\.fiducia\.cloud\/slack\/commands\/ores-chatgpt/g) ?? []).length,
    3,
  );
  assert.match(manifest, /request_url: https:\/\/api\.fiducia\.cloud\/slack\/interactions/);
  assert.match(manifest, /token_rotation_enabled: true/);
});

test('focused workflow pins exact source and credential-free checkout', () => {
  assert.match(workflow, new RegExp(`ref: ${SOURCE_SHA}`));
  assert.match(workflow, /submodules: false/);
  assert.match(workflow, /prepare-credential-free-build\.sh/);
  assert.match(workflow, new RegExp(IMAGE.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));
  assert.doesNotMatch(workflow, /submodules: recursive/);
  assert.doesNotMatch(workflow, /ubuntu-latest/);
  for (const match of workflow.matchAll(/^\s*uses:\s*([^\s#]+)/gm)) {
    assert.match(match[1], /^(?:\.\/|[^@]+@[0-9a-fA-F]{40})$/);
  }
});

const audit = {
  generated_at: new Date().toISOString(),
  source_sha: SOURCE_SHA,
  release_workflow_run: Number(WORKFLOW_RUN),
  image: IMAGE,
  resource_inventory: documents.map(identity).filter(Boolean).sort(),
  channel_bindings: registry.bindings.length,
  commands: 6,
  exact_ingress_paths: 3,
  dry_run: envBlock(deployment, 'SLACK_COMMAND_DRY_RUN').includes('value: "true"'),
  immutable_runtime: !/(?:git clone|cargo build|GH_PAT|hostPath:)/.test(deployment),
};

const auditPath = process.env.SLACK_COMMAND_GITOPS_AUDIT_PATH;
if (auditPath) {
  writeFileSync(auditPath, `${JSON.stringify(audit, null, 2)}\n`);
}
