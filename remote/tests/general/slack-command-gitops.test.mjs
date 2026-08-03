import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';

const bundlePath =
  'remote/argocd/dd-next-runtime/dd-ai-agent-bridge.service.yaml';
const kustomizationPath = 'remote/argocd/dd-next-runtime/kustomization.yaml';
const workflowPath = '.github/workflows/slack-command-gitops.yml';
const sourceRoot =
  process.env.SLACK_COMMAND_SOURCE_ROOT || '.tmp/ai-agent-bridge';
const registryPath = join(sourceRoot, 'config/alex-main-agent.channels.json');
const slackManifestPath = join(sourceRoot, 'slack-app/manifest.yaml');

const bundle = readFileSync(bundlePath, 'utf8');
const kustomization = readFileSync(kustomizationPath, 'utf8');
const workflow = readFileSync(workflowPath, 'utf8');
const registryText = readFileSync(registryPath, 'utf8');
const slackManifest = readFileSync(slackManifestPath, 'utf8');
const registry = JSON.parse(registryText);
const documents = bundle
  .split(/\n---\n/)
  .map((document) => document.trim())
  .filter(Boolean);

const sourceRevision = '7f1ad57126231c0a27da19799a27aa71f4ffaf5d';
const appId = 'A0BMBAMM5NJ';
const teamId = 'T01B3C83PMK';
const runProjectId = '72e891e2-603d-4903-8d08-bd06d204520f';

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
  return {
    kind,
    name: nameLine.replace(/^  name:\s*/, '').trim(),
  };
}

function resourceDocument(kind, name) {
  const document = documents.find((candidate) => {
    const identity = resourceIdentity(candidate);
    return identity.kind === kind && identity.name === name;
  });
  assert.ok(document, `${kind}/${name} is missing from ${bundlePath}`);
  return document;
}

function envBlock(name) {
  const marker = `            - name: ${name}\n`;
  const start = bundle.indexOf(marker);
  assert.notEqual(start, -1, `${name} is missing from ${bundlePath}`);
  const next = bundle.indexOf('\n            - name: ', start + marker.length);
  return bundle.slice(start, next === -1 ? bundle.length : next);
}

function assertSecretEnv(name) {
  const block = envBlock(name);
  assert.match(block, /valueFrom:/);
  assert.match(block, /secretKeyRef:/);
  assert.match(block, /name: dd-slack-command-secrets/);
  assert.match(block, new RegExp(`key: ${name}`));
  assert.doesNotMatch(block, /optional:\s*true/);
  assert.doesNotMatch(block, /\n\s+value:/);
}

test('bundle inventory is exact and preserves the bridge Service byte-for-byte', () => {
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
  ports:
    - name: http
      port: 8142
      targetPort: http
    - name: tcp
      port: 8143
      targetPort: tcp`;
  const bridgeService = documents[0].replace(/^(?:#.*\n)+/, '');

  assert.equal(documents.length, 8);
  assert.equal(bridgeService, expectedBridgeService);
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
  assert.equal(
    count(kustomization, '- dd-ai-agent-bridge.service.yaml'),
    1,
    'the multi-document bundle must be applied exactly once',
  );
});

test('runtime is pinned to the reviewed command source and starts in dry-run', () => {
  assert.equal(count(bundle, `value: ${sourceRevision}`), 1);
  assert.equal(count(bundle, `dd.dev/source-revision: ${sourceRevision}`), 1);
  assert.match(bundle, /fetch \\\n\s+--depth=1 origin "\$\{SLACK_COMMAND_SOURCE_REVISION\}"/);
  assert.match(bundle, /actual_revision=.*rev-parse HEAD/);
  assert.match(bundle, /source revision mismatch/);
  assert.match(bundle, /cargo build --release --locked --bin fiducia-slack-command/);
  assert.match(bundle, /exec "\$\{binary\}"/);
  assert.match(envBlock('SLACK_COMMAND_DRY_RUN'), /value: "true"/);
  assert.doesNotMatch(
    bundle,
    /SLACK_COMMAND_SOURCE_REVISION[\s\S]{0,120}value:\s*(?:main|dev|latest)\b/,
  );
});

test('source acquisition is immutable and independent of ambient GitHub credentials', () => {
  const deployment = resourceDocument('Deployment', 'dd-slack-command');
  assert.match(
    deployment,
    /image: rust:1\.97\.1-bookworm@sha256:[0-9a-f]{64}/,
  );
  assert.match(
    deployment,
    /value: https:\/\/github\.com\/ORESoftware\/ai-agent-bridge\.rs\.git/,
  );
  assert.doesNotMatch(deployment, /\b(?:GH_PAT|GITHUB_TOKEN|github_token)\b/);
  assert.doesNotMatch(deployment, /hostPath:/);
  assert.doesNotMatch(deployment, /imagePullPolicy:\s*Always/);
  assert.doesNotMatch(deployment, /image:\s*[^\n]+:(?:latest|main|dev)\b/);
});

test('installed Slack app identity and project routing remain fail-closed', () => {
  assert.match(envBlock('SLACK_EXPECTED_APP_ID'), new RegExp(`value: ${appId}`));
  assert.match(envBlock('SLACK_EXPECTED_TEAM_ID'), new RegExp(`value: ${teamId}`));
  assert.match(
    envBlock('SLACK_LINEAR_RUN_PROJECT_ID'),
    new RegExp(`value: ${runProjectId}`),
  );
  assert.match(bundle, /export SLACK_PROJECT_REGISTRY_PATH="\$\{registry\}"/);
});

test('all runtime credentials are required secret references', () => {
  for (const name of [
    'SLACK_BOT_TOKEN',
    'SLACK_SIGNING_SECRET',
    'SLACK_BRIDGE_BEARER',
    'SLACK_COORDINATOR_BEARER',
  ]) {
    assertSecretEnv(name);
    assert.match(bundle, new RegExp(`secretKey: ${name}`));
  }
  assert.match(bundle, /key: dd\/remote-dev\/alex-main-agent-slack/);
  assert.match(bundle, /key: dd\/remote-dev\/ai-agent-bridge-secrets/);
  assert.match(bundle, /key: dd\/remote-dev\/ai-agent-coordinator-secrets/);
  assert.doesNotMatch(bundle, /\b(?:xox[baprs]-|gh[pousr]_|sk-[A-Za-z0-9])/);
  assert.doesNotMatch(bundle, /SLACK_CONFIG_TOKEN/);
});

test('ExternalSecret mappings are exact, property-scoped, and never bulk-imported', () => {
  const externalSecret = resourceDocument(
    'ExternalSecret',
    'dd-slack-command-secrets',
  );
  assert.equal(count(externalSecret, 'secretKey:'), 4);
  assert.equal(count(externalSecret, 'remoteRef:'), 4);
  assert.equal(
    count(externalSecret, 'key: dd/remote-dev/alex-main-agent-slack'),
    2,
  );
  assert.equal(
    count(externalSecret, 'key: dd/remote-dev/ai-agent-bridge-secrets'),
    1,
  );
  assert.equal(
    count(externalSecret, 'key: dd/remote-dev/ai-agent-coordinator-secrets'),
    1,
  );
  for (const property of [
    'SLACK_BOT_TOKEN',
    'SLACK_SIGNING_SECRET',
    'inbox_token',
    'COORDINATOR_API_TOKEN',
  ]) {
    assert.equal(count(externalSecret, `property: ${property}`), 1);
  }
  assert.doesNotMatch(externalSecret, /\bdataFrom:/);
  assert.doesNotMatch(externalSecret, /creationPolicy:\s*(?:Merge|None)/);
});

test('only the three exact signed Slack routes are publicly exposed', () => {
  const ingress = resourceDocument('Ingress', 'dd-slack-command');
  const routes = [
    '/slack/commands/ores-claude',
    '/slack/commands/ores-chatgpt',
    '/slack/interactions',
  ];
  for (const route of routes) {
    assert.equal(count(ingress, `- path: ${route}\n`), 1);
  }
  assert.equal(count(ingress, 'pathType: Exact'), 3);
  assert.match(ingress, /host: api\.fiducia\.cloud/);
  assert.match(ingress, /secretName: gateway-public-tls/);
  assert.doesNotMatch(ingress, /- path: \/(?:\n|slack(?:\/)?\n)/);
  assert.doesNotMatch(ingress, /pathType: Prefix/);
  assert.doesNotMatch(ingress, /path: \/(?:healthz|readyz)/);
});

test('service, probes, state, pod security, and monitoring labels are explicit', () => {
  assert.match(bundle, /name: dd-slack-command[\s\S]*?type: ClusterIP/);
  assert.match(bundle, /port: 8151\n\s+targetPort: http/);
  assert.match(bundle, /containerPort: 8151/);
  assert.match(bundle, /startupProbe:[\s\S]*?path: \/healthz/);
  assert.match(bundle, /readinessProbe:[\s\S]*?path: \/readyz/);
  assert.match(bundle, /livenessProbe:[\s\S]*?path: \/healthz/);
  assert.match(bundle, /claimName: dd-slack-command-state/);
  assert.match(bundle, /storageClassName: dd-block/);
  assert.match(bundle, /strategy:\n\s+type: Recreate/);
  assert.match(bundle, /automountServiceAccountToken: false/);
  assert.match(bundle, /allowPrivilegeEscalation: false/);
  assert.match(bundle, /readOnlyRootFilesystem: true/);
  assert.match(bundle, /runAsNonRoot: true/);
  assert.match(bundle, /capabilities:\n\s+drop:\n\s+- ALL/);
  assert.match(bundle, /seccompProfile:\n\s+type: RuntimeDefault/);
  assert.match(bundle, /resources:\n\s+requests:[\s\S]*?limits:/);
  assert.match(
    bundle,
    /kind: Deployment\nmetadata:\n  name: dd-slack-command[\s\S]*?labels:\n    app: dd-ai-agent-bridge\n    app\.kubernetes\.io\/component: slack-command/,
  );
  assert.match(
    bundle,
    /kind: Deployment[\s\S]*?selector:\n    matchLabels:\n      app: dd-ai-agent-bridge\n      app\.kubernetes\.io\/component: slack-command/,
  );
  assert.match(
    bundle,
    /kind: Service\nmetadata:\n  name: dd-slack-command[\s\S]*?selector:\n    app: dd-ai-agent-bridge\n    app\.kubernetes\.io\/component: slack-command/,
  );
  assert.equal(count(bundle, 'app.kubernetes.io/component: slack-command'), 6);
});

test('state and rollout contracts preserve one durable journal writer', () => {
  const deployment = resourceDocument('Deployment', 'dd-slack-command');
  const pvc = resourceDocument(
    'PersistentVolumeClaim',
    'dd-slack-command-state',
  );
  const pdb = resourceDocument('PodDisruptionBudget', 'dd-slack-command');
  assert.match(deployment, /replicas: 1/);
  assert.match(deployment, /strategy:\n\s+type: Recreate/);
  assert.match(
    envBlock('SLACK_COMMAND_STATE_DIR'),
    /value: \/var\/lib\/slack-command\/runs/,
  );
  assert.match(
    deployment,
    /mountPath: \/var\/lib\/slack-command[\s\S]*?claimName: dd-slack-command-state/,
  );
  assert.match(pvc, /accessModes:\n\s+- ReadWriteOnce/);
  assert.match(pvc, /storage: 1Gi/);
  assert.match(pdb, /minAvailable: 1/);
});

test('network policy limits ingress and grants only required egress classes', () => {
  const policy = resourceDocument('NetworkPolicy', 'dd-slack-command');
  assert.match(
    policy,
    /kubernetes\.io\/metadata\.name: ingress-nginx[\s\S]*?port: 8151/,
  );
  assert.match(policy, /app: dd-ai-agent-bridge[\s\S]*?port: 8142/);
  assert.match(
    policy,
    /kubernetes\.io\/metadata\.name: ai-agent-coordinator[\s\S]*?port: 8080/,
  );
  assert.match(policy, /kubernetes\.io\/metadata\.name: kube-system/);
  assert.match(policy, /cidr: 0\.0\.0\.0\/0/);
  assert.match(policy, /port: 443/);
  assert.doesNotMatch(policy, /port:\s*(?:22|80|6443)\b/);
  assert.doesNotMatch(policy, /namespaceSelector:\s*\{\}/);
  assert.doesNotMatch(policy, /podSelector:\s*\{\}/);
});

test('the reviewed source contains exactly thirteen bounded channel bindings', () => {
  assert.equal(registry.schema_version, 1);
  assert.equal(registry.bindings.length, 13);

  const channels = new Set();
  const projects = new Set();
  const repositories = new Set();
  for (const binding of registry.bindings) {
    assert.equal(binding.workspace_id, teamId);
    assert.equal(binding.linear_team_key, 'DEN');
    assert.deepEqual(binding.allowed_user_ids, ['U01AZNU2LJ2']);
    assert.deepEqual(binding.allowed_user_group_ids, []);
    assert.equal(binding.write_policy, 'draft_pull_request');
    assert.equal(binding.default_repository, binding.repository_allowlist[0]);
    assert.equal(binding.repository_allowlist.length, 1);
    assert.equal(binding.budget_policy.max_concurrent_runs, 4);
    assert.equal(binding.budget_policy.max_runtime_secs, 900);
    assert.equal(binding.budget_policy.max_retries, 2);
    channels.add(binding.channel_id);
    projects.add(binding.linear_project_id);
    repositories.add(binding.default_repository);
  }

  assert.equal(channels.size, 13);
  assert.equal(projects.size, 13);
  assert.equal(repositories.size, 13);
  assert.deepEqual(
    [...repositories].sort(),
    [
      '3FA-app/3FA-mcp-server.rs',
      'StreemPilot/streempilot-mcp-server.rs',
      'athlet-o/athleto-mcp-server.rs',
      'benefactor-cc/benefactor-cc-mcp-server.rs',
      'cliptown/cliptown-monorepo',
      'daedalus-fab/daedalus-fab-mcp-server.rs',
      'hypesiege/hypesiege-mcp-server.rs',
      'memebank/mbk-api',
      'opto-sync/syncer.rs',
      'quaestor-ledger/quaestor-ledger-mcp-server.rs',
      'scintilla-run/scintilla-mcp-server.rs',
      'shared-auth/shared-auth-mcp-server.rs',
      'voxletra/vxl-api-server.rs',
    ].sort(),
  );
});

test('the source Slack manifest and deployed ingress cannot drift', () => {
  for (const command of [
    '/ores-claude',
    '/ores-chatgpt',
    '/x-claude',
    '/x-chatgpt',
    '/my-claude',
    '/my-chatgpt',
  ]) {
    assert.equal(count(slackManifest, `command: ${command}`), 1);
  }
  assert.equal(
    count(
      slackManifest,
      'https://api.fiducia.cloud/slack/commands/ores-claude',
    ),
    3,
  );
  assert.equal(
    count(
      slackManifest,
      'https://api.fiducia.cloud/slack/commands/ores-chatgpt',
    ),
    3,
  );
  assert.equal(
    count(slackManifest, 'https://api.fiducia.cloud/slack/interactions'),
    1,
  );
  assert.match(slackManifest, /- commands/);
  assert.match(slackManifest, /token_rotation_enabled: true/);
  assert.doesNotMatch(slackManifest, /\b(?:xox[baprs]-|gh[pousr]_)/);
});

test('GitHub Actions tracks the complete contract and pins every dependency', () => {
  assert.match(workflow, /pull_request:\n\s+branches: \[dev\]/);
  assert.match(workflow, /push:\n\s+branches: \[dev\]/);
  for (const path of [
    '.github/workflows/slack-command-gitops.yml',
    'docs/alex-main-agent-slack-command-gitops.md',
    bundlePath,
    'remote/tests/general/slack-command-gitops.test.mjs',
  ]) {
    assert.equal(count(workflow, `'${path}'`), 2, `${path} must trigger PR and push CI`);
  }
  for (const match of workflow.matchAll(/^\s*uses:\s*([^\s#]+)/gm)) {
    const reference = match[1];
    assert.match(
      reference,
      /@[0-9a-f]{40}$/,
      `workflow dependency must be immutable: ${reference}`,
    );
  }
  assert.doesNotMatch(workflow, /runs-on:\s*ubuntu-latest/);
  assert.match(workflow, new RegExp(`ref: ${sourceRevision}`));
  assert.doesNotMatch(workflow, /agent\/den-1041-ores-slash-commands/);
});

const auditPath = process.env.SLACK_COMMAND_GITOPS_AUDIT_PATH;
if (auditPath) {
  const audit = {
    generated_at: new Date().toISOString(),
    issue: 'DEN-1298',
    source_revision: sourceRevision,
    runtime: {
      app_id: appId,
      workspace_id: teamId,
      dry_run: true,
      manifest_documents: documents.map(resourceIdentity),
      public_routes: [
        '/slack/commands/ores-claude',
        '/slack/commands/ores-chatgpt',
        '/slack/interactions',
      ],
      state: 'dd-slack-command-state',
      registry_bindings: registry.bindings.length,
    },
    activation_gates: {
      external_secret_ready: false,
      remote_manifest_reconciled: false,
      app_reinstalled: false,
      signed_canary_passed: false,
      live_dispatch_enabled: false,
    },
  };
  writeFileSync(auditPath, `${JSON.stringify(audit, null, 2)}\n`);
}
