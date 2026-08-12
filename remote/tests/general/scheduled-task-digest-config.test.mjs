import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

async function text(relative) {
  return readFile(new URL(relative, import.meta.url), 'utf8');
}

test('AWS-only Application owns the digest and prevents a mirrored Hetzner sender', async () => {
  const application = await text('../../argocd/clusters/aws/scheduled-task-digest.application.yaml');
  const awsKustomization = await text('../../argocd/clusters/aws/kustomization.yaml');
  assert.match(application, /name: dd-scheduled-task-digest/);
  assert.match(application, /targetRevision: dev/);
  assert.match(application, /path: remote\/argocd\/scheduled-task-digest/);
  assert.match(awsKustomization, /scheduled-task-digest\.application\.yaml/);
});

test('CronJob is an unsuspended exact 07:00 America\/Chicago schedule with one fixed recipient', async () => {
  const cron = await text('../../argocd/scheduled-task-digest/cronjob.yaml');
  const source = (await Promise.all([
    text('../../argocd/scheduled-task-digest/shared.mjs'),
    text('../../argocd/scheduled-task-digest/github.mjs'),
    text('../../argocd/scheduled-task-digest/kubernetes.mjs'),
    text('../../argocd/scheduled-task-digest/digest.mjs'),
    text('../../argocd/scheduled-task-digest/main.mjs'),
  ])).join('\n');
  assert.match(cron, /schedule: "0 7 \* \* \*"/);
  assert.match(cron, /timeZone: America\/Chicago/);
  assert.match(cron, /suspend: false/);
  assert.match(cron, /concurrencyPolicy: Forbid/);
  assert.match(cron, /dd\.dev\/evidence-window: 24h/);
  assert.match(cron, /dd\.dev\/digest\.exclude: "true"/);
  assert.match(source, /recipient: 'alexander\.d\.mills@gmail\.com'/);
  assert.match(source, /lookbackHours: 24/);
});

test('runtime reads every Kubernetes CronJob and the central GitHub schedule repositories', async () => {
  const source = (await Promise.all([
    text('../../argocd/scheduled-task-digest/shared.mjs'),
    text('../../argocd/scheduled-task-digest/github.mjs'),
    text('../../argocd/scheduled-task-digest/kubernetes.mjs'),
    text('../../argocd/scheduled-task-digest/digest.mjs'),
    text('../../argocd/scheduled-task-digest/main.mjs'),
  ])).join('\n');
  assert.match(source, /\/apis\/batch\/v1\/cronjobs/);
  assert.match(source, /\/apis\/batch\/v1\/jobs/);
  for (const repository of [
    'ORESoftware/ai-agent-coordinator.rs',
    'ORESoftware/k8s-cluster',
    'ORESoftware/project-registry',
  ]) {
    assert.match(source, new RegExp(repository.replaceAll('.', '\\.')));
  }
});

test('mailer uses the authenticated internal contact service and a Kubernetes Lease', async () => {
  const cron = await text('../../argocd/scheduled-task-digest/cronjob.yaml');
  const rbac = await text('../../argocd/scheduled-task-digest/rbac.yaml');
  const source = (await Promise.all([
    text('../../argocd/scheduled-task-digest/shared.mjs'),
    text('../../argocd/scheduled-task-digest/github.mjs'),
    text('../../argocd/scheduled-task-digest/kubernetes.mjs'),
    text('../../argocd/scheduled-task-digest/digest.mjs'),
    text('../../argocd/scheduled-task-digest/main.mjs'),
  ])).join('\n');
  assert.match(cron, /dd-email-sms-contact-rs\.default\.svc\.cluster\.local:8120/);
  assert.match(cron, /name: SERVER_AUTH_SECRET[\s\S]*name: dd-agent-secrets[\s\S]*key: SERVER_AUTH_SECRET/);
  assert.match(rbac, /apiGroups:[\s\S]*coordination\.k8s\.io[\s\S]*resources:[\s\S]*leases/);
  assert.doesNotMatch(rbac, /verbs:\s*\n\s*- "?\*"?/);
  assert.match(source, /duplicate_suppressed/);
  assert.match(source, /dd-scheduled-task-digest-delivery/);
});

test('pod security and supply-chain boundaries are explicit', async () => {
  const cron = await text('../../argocd/scheduled-task-digest/cronjob.yaml');
  assert.match(cron, /node:24\.18\.0-alpine3\.24@sha256:[a-f0-9]{64}/);
  assert.match(cron, /runAsNonRoot: true/);
  assert.match(cron, /readOnlyRootFilesystem: true/);
  assert.match(cron, /allowPrivilegeEscalation: false/);
  assert.match(cron, /capabilities:[\s\S]*drop:[\s\S]*- ALL/);
  assert.match(cron, /seccompProfile:[\s\S]*type: RuntimeDefault/);
});

test('production files contain no credential-shaped literals', async () => {
  const contents = await Promise.all([
    text('../../argocd/scheduled-task-digest/shared.mjs'),
    text('../../argocd/scheduled-task-digest/github.mjs'),
    text('../../argocd/scheduled-task-digest/kubernetes.mjs'),
    text('../../argocd/scheduled-task-digest/digest.mjs'),
    text('../../argocd/scheduled-task-digest/main.mjs'),
    text('../../argocd/scheduled-task-digest/cronjob.yaml'),
    text('../../argocd/scheduled-task-digest/rbac.yaml'),
    text('../../argocd/clusters/aws/scheduled-task-digest.application.yaml'),
  ]);
  const joined = contents.join('\n');
  assert.doesNotMatch(joined, /ghp_[A-Za-z0-9]{20,}/);
  assert.doesNotMatch(joined, /lin_api_[A-Za-z0-9]{20,}/);
  assert.doesNotMatch(joined, /SG\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}/);
  assert.doesNotMatch(joined, /-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----/);
});
