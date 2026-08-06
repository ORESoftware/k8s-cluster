import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const read = (path) => readFile(path, 'utf8');

test('telemetry foundation preserves standalone coordinator ownership', async () => {
  const runtime = await read('remote/argocd/dd-next-runtime/kustomization.yaml');
  const application = await read('remote/argocd/apps/ai-agent-coordinator.application.yaml');
  const broker = await read('remote/argocd/dd-next-runtime/dd-dev-server-home.deployment.yaml');

  assert.doesNotMatch(runtime, /dd-ai-agent-runner/);
  assert.match(application, /ORESoftware\/ai-agent-coordinator\.rs\.git/);
  assert.match(application, /targetRevision:\s*(?:ai-agent-coordinator-rs-v\d+\.\d+\.\d+|[0-9a-f]{40})/);
  assert.match(broker, /name:\s*GEMINI_MODEL[\s\S]*value:\s*gemini-3\.1-pro-preview/);
  assert.match(broker, /name:\s*ANTHROPIC_MODEL[\s\S]*value:\s*claude-opus-5/);
  assert.match(broker, /name:\s*CODEX_MODEL[\s\S]*value:\s*gpt-5\.6-sol/);
});

test('Loki and OTEL produce bounded remediation signals without activating secret delivery', async () => {
  const kustomization = await read('remote/argocd/observability/kustomization.yaml');
  const loki = await read('remote/argocd/observability/loki.configmap.yaml');
  const lokiDeployment = await read('remote/argocd/observability/loki.deployment.yaml');
  const rules = await read('remote/argocd/observability/loki.rules.configmap.yaml');
  const collector = await read('remote/argocd/observability/otel-collector.configmap.yaml');

  assert.match(kustomization, /- loki\.rules\.configmap\.yaml/);
  assert.match(loki, /ruler:[\s\S]*alertmanager_url:/);
  assert.match(lokiDeployment, /name:\s*dd-loki-rules/);
  assert.match(rules, /alert:\s*DDBackendErrorLogBurst/);
  assert.match(rules, /alert:\s*DDBackendWarningLogBurst/);
  assert.match(rules, /log_schema="dd\.log\.v1"/);
  assert.doesNotMatch(rules, /trace[_-]?id|request[_-]?id|task[_-]?id|user[_-]?id/i);
  assert.match(collector, /connectors:[\s\S]*spanmetrics:/);
  assert.match(collector, /receivers:[\s\S]*-\s*spanmetrics/);
  assert.doesNotMatch(kustomization, /alertmanager\.telemetry\.externalsecret\.yaml/);
});

test('instrumented workloads can reach OTLP and promotion remains least-privilege and draft-only', async () => {
  const paths = [
    'remote/argocd/dd-next-runtime/dd-rust-network-mutex-mills.statefulset.yaml',
    'remote/argocd/dd-next-runtime/dd-t2v-api.deployment.yaml',
    'remote/argocd/dd-next-runtime/dd-t2v-web.deployment.yaml',
    'remote/deployments/gcs/k8s/ec2/gcs.deployment.yaml',
  ];
  for (const path of paths) {
    const manifest = await read(path);
    assert.match(manifest, /OTEL_EXPORTER_OTLP_ENDPOINT|otel-collector/);
  }

  for (const path of [
    'remote/argocd/dd-next-runtime/dd-t2v-api.networkpolicy.yaml',
    'remote/argocd/dd-next-runtime/dd-t2v-web.networkpolicy.yaml',
  ]) {
    const policy = await read(path);
    assert.match(policy, /namespaceSelector:[\s\S]*observability/);
    assert.match(policy, /port:\s*4317|port:\s*4318/);
  }

  const workflow = await read('.github/workflows/telemetry-upstream-promotion.yml');
  assert.match(workflow, /repository_dispatch/);
  assert.match(workflow, /draft:\s*true|--draft/);
  assert.doesNotMatch(workflow, /gh\s+pr\s+merge|merge_pull_request|push\s+origin\s+(?:dev|main)/i);

  assert.doesNotMatch(workflow, /ORG_GITOPS_TOKEN|personal[_-]?access[_-]?token/i);
  assert.match(workflow, /K8S_SUBMODULE_APP_ID/);
  assert.match(workflow, /K8S_SUBMODULE_APP_PRIVATE_KEY/);
  assert.match(workflow, /mint-github-app-installation-token\.sh/);
  assert.match(workflow, /GH_TOKEN:\s*\$\{\{\s*github\.token\s*\}\}/);
  assert.match(workflow, /Authorization: Bearer \$\{upstream_token\}/);
  assert.match(workflow, /request DELETE[\s\S]*\/installation\/token/);
  assert.match(workflow, /contents:\s*write[\s\S]*pull-requests:\s*write/);
});
