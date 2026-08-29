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

const SOURCE_SHA = 'c3e54e6cd0c6d56e3d2ed32902228d974e550a3f';
const WORKFLOW_RUN = '31235992249';
const IMAGE =
  'ghcr.io/oresoftware/fiducia-slack-command@sha256:01f80fbd4d3ba5226b4abdb7f5e603538924edb48e79e72b0af43246624900cb';

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
  const paths = [...ingress.matchAll(/^\s*-\s+path:\s*(\S+)\s*$/gm)].map((match) => match[1]);
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

test('reviewed source retains fourteen bounded channel bindings', () => {
  assert.equal(registry.schema_version, 1);
  assert.equal(registry.bindings.length, 14);
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
  assert.doesNotMatch(workflow, /^\s*runs-on:\s*ubuntu-latest\s*$/m);
  for (const match of workflow.matchAll(/^\s*uses:\s*([^\s#]+)/gm)) {
    assert.match(
      match[1],
      /^(?:\.\/|[^@]+@[0-9a-fA-F]{40}|docker:\/\/[^@\s]+@sha256:[0-9a-fA-F]{64})$/,
    );
  }
});

// ---------------------------------------------------------------------------
// Salvaged from agent/DEN-1298-slack-command-gitops.
// These assertions are orthogonal to how the binary is delivered: they survived
// the DEN-845 cutover from an in-pod `cargo build` to a digest-pinned image and
// were never ported. Everything below is strictly additive to the contracts
// above; nothing here relaxes an existing assertion.
// ---------------------------------------------------------------------------

function count(text, needle) {
  return text.split(needle).length - 1;
}

function resourceIdentity(document) {
  const kind = document.match(/^kind:\s*([^\s]+)\s*$/m)?.[1];
  const lines = document.split('\n');
  const metadataIndex = lines.findIndex((line) => line === 'metadata:');
  assert.notEqual(metadataIndex, -1, 'resource document is missing metadata');
  const nameLine = lines
    .slice(metadataIndex + 1)
    .find((line) => /^  name:\s*\S+\s*$/.test(line));
  assert.ok(kind, 'resource document is missing kind');
  assert.ok(nameLine, 'resource document is missing metadata.name');
  return { kind, name: nameLine.replace(/^  name:\s*/, '').trim() };
}

function assertSecretEnv(name) {
  const block = envBlock(deployment, name);
  assert.match(block, /valueFrom:/);
  assert.match(block, /secretKeyRef:/);
  assert.match(block, /name: dd-slack-command-secrets/);
  assert.match(block, new RegExp(`key: ${name}`));
  // A credential secretKeyRef marked optional degrades silently to an unset
  // variable: the pod starts, signature verification is misconfigured, and
  // nothing fails loudly. Required means required.
  assert.doesNotMatch(block, /optional:\s*true/);
  assert.doesNotMatch(block, /\n\s+value:/);
}

// S4 - the bundle is a single multi-document file, so document order is part of
// the contract, not just the set of identities.
test('salvage: bundle inventory is exact in both count and order', () => {
  assert.equal(documents.length, 8);
  assert.deepEqual(
    documents.map(resourceIdentity),
    [
      { kind: 'Service', name: 'dd-ai-agent-bridge' },
      { kind: 'ExternalSecret', name: 'dd-slack-command-secrets' },
      { kind: 'PersistentVolumeClaim', name: 'dd-slack-command-state' },
      { kind: 'Deployment', name: 'dd-slack-command' },
      { kind: 'Service', name: 'dd-slack-command' },
      { kind: 'NetworkPolicy', name: 'dd-slack-command' },
      { kind: 'PodDisruptionBudget', name: 'dd-slack-command' },
      { kind: 'Ingress', name: 'dd-slack-command' },
    ],
  );
});

// S5 - the Slack resources were appended to a file that already held an
// unrelated Service. Pin that neighbour byte-for-byte so an edit to another
// team's resource cannot ride along inside this bundle.
test('salvage: the pre-existing bridge Service is preserved byte-for-byte', () => {
  const expectedBridgeService = `apiVersion: v1
kind: Service
metadata:
  name: dd-ai-agent-bridge
  namespace: default
  labels:
    app: dd-ai-agent-bridge
spec:
  selector:
    app: dd-ai-agent-bridge
    app.kubernetes.io/component: bridge
  ports:
    - name: http
      port: 8142
      targetPort: http
    - name: tcp
      port: 8143
      targetPort: tcp`;
  assert.equal(documents[0].replace(/^(?:#.*\n)+/, ''), expectedBridgeService);
});

// S6 - the existing ingress test asserts only what must be present. A single
// added `pathType: Prefix` rule would publish the whole service while the three
// exact paths remain present and `pathType: Exact` still appears three times,
// so every positive assertion above would still pass.
test('salvage: the public ingress cannot widen beyond three exact routes', () => {
  assert.match(ingress, /secretName: gateway-public-tls/);
  assert.doesNotMatch(ingress, /pathType: Prefix/);
  assert.doesNotMatch(ingress, /path: \/(?:healthz|readyz)/);
});

// S7 - the four-property allowlist is the whole secret-scoping model. Nothing
// currently stops it becoming a bulk import.
test('salvage: ExternalSecret stays a four-property allowlist, never bulk-imported', () => {
  assert.equal(count(externalSecret, 'secretKey:'), 4);
  assert.equal(count(externalSecret, 'remoteRef:'), 4);
  // dataFrom: would pull an entire Secrets Manager bundle into the pod env.
  assert.doesNotMatch(externalSecret, /\bdataFrom:/);
  assert.doesNotMatch(externalSecret, /creationPolicy:\s*(?:Merge|None)/);
  for (const property of [
    'SLACK_BOT_TOKEN',
    'SLACK_SIGNING_SECRET',
    'inbox_token',
    'COORDINATOR_API_TOKEN',
  ]) {
    assert.equal(count(externalSecret, `property: ${property}`), 1);
  }
});

// S8 - required-ness of every credential reference.
test('salvage: credential env refs are required, never optional or inline', () => {
  for (const name of [
    'SLACK_BOT_TOKEN',
    'SLACK_SIGNING_SECRET',
    'SLACK_BRIDGE_BEARER',
    'SLACK_COORDINATOR_BEARER',
  ]) {
    assertSecretEnv(name);
  }
});

// S9 - the existing literal scan is scoped to the ExternalSecret document only.
test('salvage: no credential-shaped literal appears anywhere in the bundle', () => {
  assert.doesNotMatch(bundle, /\b(?:xox[baprs]-|gh[pousr]_|sk-[A-Za-z0-9])/);
  // An app-configuration token is an admin credential and must never become a
  // runtime environment variable.
  assert.doesNotMatch(bundle, /SLACK_CONFIG_TOKEN/);
  assert.doesNotMatch(manifest, /\b(?:xox[baprs]-|gh[pousr]_)/);
  assert.doesNotMatch(workflow, /agent\/den-1041-ores-slash-commands/);
});

// S10 - the existing registry test proves no channel is bound twice. It does not
// prove the converse blast-radius property: that one channel cannot dispatch
// work into another channel's Linear project or repository.
test('salvage: no two channels share a Linear project or a repository', () => {
  const expected = registry.bindings.length;
  const channels = new Set();
  const projects = new Set();
  const repositories = new Set();
  for (const binding of registry.bindings) {
    assert.equal(
      typeof binding.linear_project_id,
      'string',
      `binding ${binding.channel_id} is missing linear_project_id`,
    );
    assert.equal(
      typeof binding.default_repository,
      'string',
      `binding ${binding.channel_id} is missing default_repository`,
    );
    assert.deepEqual(binding.allowed_user_group_ids, []);
    channels.add(binding.channel_id);
    projects.add(binding.linear_project_id);
    repositories.add(binding.default_repository);
  }
  assert.equal(channels.size, expected, 'two bindings share a channel');
  assert.equal(projects.size, expected, 'two bindings share a Linear project');
  assert.equal(repositories.size, expected, 'two bindings share a repository');
});

// S3 - trigger accounting. Catches the classic bug where a path is added to
// pull_request.paths and forgotten in push.paths.
//
// NOTE: this is deliberately a structural parse of the `on:` block rather than
// the simpler "each path string occurs exactly twice" count used on the source
// branch. That count is wrong against main: the workflow names its own file a
// third time inside the "Reject mutable workflow dependencies" step
// (`path = Path('.github/workflows/slack-command-gitops.yml')`), so a bare
// occurrence count reads 3 and fails on a correct workflow.
test('salvage: every contract file triggers both PR and push CI', () => {
  function triggerPaths(event) {
    const block = workflow.match(
      new RegExp(`^  ${event}:\\n(?:    .*\\n)+`, 'm'),
    );
    assert.ok(block, `workflow is missing the ${event} trigger`);
    const paths = [...block[0].matchAll(/^      - '([^']+)'\s*$/gm)].map(
      (match) => match[1],
    );
    assert.ok(paths.length > 0, `${event} trigger lists no paths`);
    return paths;
  }

  const pullRequestPaths = triggerPaths('pull_request');
  const pushPaths = triggerPaths('push');
  assert.deepEqual(
    pullRequestPaths,
    pushPaths,
    'pull_request.paths and push.paths must stay identical',
  );

  for (const tracked of [
    workflowPath,
    bundlePath,
    kustomizationPath,
    'remote/tests/general/slack-command-gitops.test.mjs',
    // The runbook documents this exact bundle. Before this change it was in
    // neither trigger list, so it could drift from the manifests silently.
    'docs/alex-main-agent-slack-command-gitops.md',
  ]) {
    assert.ok(
      pullRequestPaths.includes(tracked),
      `${tracked} must trigger the contract workflow`,
    );
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
