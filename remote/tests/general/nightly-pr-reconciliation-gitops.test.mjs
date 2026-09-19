import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

const sourceRevision = '4bc6306928e6b7d2fb300ffca07a1bb667a90242';
const credentialPattern = new RegExp([
  ['gh', 'p_'].join(''),
  ['github', '_pat_'].join(''),
  'personal.?access.?token',
].join('|'), 'i');

function findRepoRoot() {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/argocd/project-automation/kustomization.yaml'))) {
      return candidate;
    }
  }
  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();
const overlay = 'remote/argocd/project-automation';

async function readRepoFile(path) {
  return readFile(resolve(repoRoot, path), 'utf8');
}

test('nightly governed reconciliation schedule is DST-aware, staged, bounded, and immutable', async () => {
  const cronjob = await readRepoFile(`${overlay}/nightly-pr-reconciliation.cronjob.yaml`);

  assert.match(cronjob, /kind: CronJob/);
  assert.match(cronjob, /name: dd-nightly-pr-reconciliation/);
  assert.match(cronjob, /schedule: "0 1 \* \* \*"/);
  assert.match(cronjob, /timeZone: America\/Chicago/);
  assert.match(cronjob, /suspend: true/);
  assert.match(cronjob, /dd\.dev\/activation-state: suspended-pending-trusted-finalizer/);
  assert.match(cronjob, /concurrencyPolicy: Forbid/);
  assert.match(cronjob, /startingDeadlineSeconds: 3600/);
  assert.match(cronjob, /activeDeadlineSeconds: 43200/);
  assert.match(cronjob, /backoffLimit: 0/);
  assert.match(cronjob, /denman\.linear\.app\/linear-reconciliation-issue: DEN-2876/);
  assert.match(cronjob, /dd\.dev\/required-linear-opinion-agents: "chatgpt,claude"/);
  assert.match(cronjob, /dd\.dev\/linear-draft-status: Backlog/);
  assert.match(cronjob, /dd\.dev\/linear-pending-status: Todo/);
  assert.match(cronjob, /dd\.dev\/readiness-threshold-exclusive: "0\.995"/);
  assert.match(cronjob, /dd\.dev\/minimum-continuous-open-hours: "55"/);
  assert.match(cronjob, new RegExp(`dd\\.dev/source-revision: ${sourceRevision}`));
  assert.match(cronjob, new RegExp(`PROJECT_REGISTRY_REVISION\\n\\s+value: ${sourceRevision}`));
  assert.match(
    cronjob,
    /image: docker\.io\/library\/node:24\.18\.0-alpine3\.24@sha256:[0-9a-f]{64}/,
  );
  assert.doesNotMatch(cronjob, /:(?:main|latest)\s*$/m);
});

test('bootstrap verifies exact source and dispatches GitHub and Linear reconciliation together', async () => {
  const config = await readRepoFile(`${overlay}/nightly-pr-reconciliation.configmap.yaml`);

  assert.match(config, /SOURCE_REPOSITORY = 'project-registry'/);
  assert.match(config, /commits\/\$\{revision\}/);
  assert.match(config, /commit\?\.sha !== revision/);
  for (const source of [
    'src/nightly-pr-reconciliation.mjs',
    'src/nightly-linear-reconciliation.mjs',
    'src/nightly-linear-reconciliation-policy.mjs',
    'src/nightly-linear-reconciliation-stage.mjs',
    'registry/nightly-pr-reconciliation.json',
    'registry/nightly-linear-reconciliation.json',
    'registry/nightly-interdependency.json'
  ]) {
    assert.match(config, new RegExp(source.replaceAll('.', '\\.')));
  }
  assert.match(config, /loadNightlyPrReconciliationPolicy/);
  assert.match(config, /loadNightlyLinearReconciliationPolicy/);
  assert.match(config, /createNightlyPrReconciliationPlan/);
  assert.match(config, /createNightlyLinearReconciliationPlan/);
  assert.match(config, /dispatchNightlyPrReconciliationPlan/);
  assert.match(config, /dispatchNightlyLinearReconciliationPlan/);
  assert.match(config, /required_linear_agents: linearPolicy\.execution\.agents/);
  assert.match(config, /linear_draft_status: linearPolicy\.statusMapping\.denman\.draft/);
  assert.match(config, /linear_pending_status: linearPolicy\.statusMapping\.denman\.pending/);
  assert.match(config, /logical_task_type: job\.request\.task_type/);
  assert.match(config, /task_type: queueTaskType/);
  assert.match(config, /queueTaskType !== 'nightly_org_maintenance'/);
  assert.match(config, /nightly_governed_reconciliation_run\.v1/);
  assert.doesNotMatch(config, credentialPattern);
});

test('credentials, pod security, and egress remain least-privilege', async () => {
  const cronjob = await readRepoFile(`${overlay}/nightly-pr-reconciliation.cronjob.yaml`);
  const networkPolicy = await readRepoFile(`${overlay}/nightly-pr-reconciliation.networkpolicy.yaml`);

  assert.match(cronjob, /name: COORDINATOR_TASK_TYPE\n\s+value: nightly_org_maintenance/);
  assert.match(cronjob, /automountServiceAccountToken: false/);
  assert.match(cronjob, /readOnlyRootFilesystem: true/);
  assert.match(cronjob, /allowPrivilegeEscalation: false/);
  assert.match(cronjob, /capabilities:\s*\n\s*drop:\s*\n\s*- ALL/);
  assert.match(cronjob, /ai-agent-coordinator\.ai-agent-coordinator\.svc\.cluster\.local:8080/);
  assert.match(networkPolicy, /kubernetes\.io\/metadata\.name: kube-system/);
  assert.match(networkPolicy, /kubernetes\.io\/metadata\.name: ai-agent-coordinator/);
  assert.match(networkPolicy, /port: 8080/);
  assert.match(networkPolicy, /port: 443/);
  assert.match(networkPolicy, /ingress: \[\]/);
  assert.doesNotMatch(`${cronjob}\n${networkPolicy}`, credentialPattern);
});

test('Kustomize includes every nightly governed reconciliation resource', async () => {
  const kustomization = await readRepoFile(`${overlay}/kustomization.yaml`);
  for (const resource of [
    'nightly-pr-reconciliation.configmap.yaml',
    'nightly-pr-reconciliation.cronjob.yaml',
    'nightly-pr-reconciliation.networkpolicy.yaml',
  ]) {
    assert.match(kustomization, new RegExp(resource.replaceAll('.', '\\.')));
  }
});
