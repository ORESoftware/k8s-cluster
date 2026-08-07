import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

const sourceRevision = 'aafd986c59699d2de1a5879f05a0fd71731c90f2';

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

test('nightly interdependency schedule is DST-aware, active, bounded, and immutable', async () => {
  const cronjob = await readRepoFile(`${overlay}/nightly-interdependency.cronjob.yaml`);

  assert.match(cronjob, /kind: CronJob/);
  assert.match(cronjob, /name: dd-nightly-interdependency/);
  assert.match(cronjob, /schedule: "0 2 \* \* \*"/);
  assert.match(cronjob, /timeZone: America\/Chicago/);
  assert.match(cronjob, /suspend: false/);
  assert.match(cronjob, /concurrencyPolicy: Forbid/);
  assert.match(cronjob, /startingDeadlineSeconds: 3600/);
  assert.match(cronjob, /activeDeadlineSeconds: 43200/);
  assert.match(cronjob, /backoffLimit: 0/);
  assert.match(cronjob, new RegExp(`dd\\.dev/source-revision: ${sourceRevision}`));
  assert.match(cronjob, new RegExp(`PROJECT_REGISTRY_REVISION\\n\\s+value: ${sourceRevision}`));
  assert.match(
    cronjob,
    /image: docker\.io\/library\/node:24\.18\.0-alpine3\.24@sha256:[0-9a-f]{64}/,
  );
  assert.doesNotMatch(cronjob, /:(?:main|latest)\s*$/m);
});

test('bootstrap verifies immutable source and runs graph preflight before organization updates', async () => {
  const config = await readRepoFile(`${overlay}/nightly-interdependency.configmap.yaml`);

  assert.match(config, /SOURCE_REPOSITORY = 'project-registry'/);
  assert.match(config, /commits\/\$\{revision\}/);
  assert.match(config, /commit\?\.sha !== revision/);
  assert.match(config, /nightly_interdependency_graph\.v1/);
  assert.match(config, /nightly_interdependency_graph_result\.v1/);
  assert.match(config, /dependency-graphs\/\$\{scheduleDate\}/);
  assert.match(config, /\.gitmodules/);
  assert.match(config, /flake\.nix/);
  assert.match(config, /flake\.lock/);
  assert.match(config, /\.zpkg\.toml/);
  assert.match(config, /\.zpkg\.lock/);
  assert.match(config, /strongly connected components/i);
  assert.match(config, /topological update waves/i);
  assert.match(config, /fleet_graph_dispatch_started/);
  assert.match(config, /organization_updates_dispatch_started/);
  assert.ok(
    config.indexOf('fleet_graph_dispatch_started') <
      config.indexOf('organization_updates_dispatch_started'),
    'fleet graph must complete before organization updates are dispatched',
  );
  assert.match(config, /MAX_GRAPH_RESULT_BYTES = 64 \* 1024/);
  assert.match(config, /Never close or rewrite a human-authored PR/);
  assert.match(config, /nightly-interdependency:fleet-graph:/);
  assert.doesNotMatch(config, /ghp_|github_pat_|personal.?access.?token/i);
});

test('credentials and egress are least-privilege and coordinator-scoped', async () => {
  const cronjob = await readRepoFile(`${overlay}/nightly-interdependency.cronjob.yaml`);
  const externalSecret = await readRepoFile(`${overlay}/externalsecret.yaml`);
  const networkPolicy = await readRepoFile(`${overlay}/nightly-interdependency.networkpolicy.yaml`);

  assert.match(externalSecret, /key: dd\/remote-dev\/project-automation-secrets/);
  assert.match(externalSecret, /property: github_app_private_key/);
  assert.match(externalSecret, /key: dd\/remote-dev\/ai-agent-coordinator-secrets/);
  assert.match(externalSecret, /property: COORDINATOR_API_TOKEN/);
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
  assert.doesNotMatch(`${cronjob}\n${externalSecret}\n${networkPolicy}`, /ghp_|github_pat_/i);
});

test('Kustomize renders all scheduler resources', async () => {
  const kustomization = await readRepoFile(`${overlay}/kustomization.yaml`);
  for (const resource of [
    'nightly-interdependency.configmap.yaml',
    'nightly-interdependency.cronjob.yaml',
    'nightly-interdependency.networkpolicy.yaml',
  ]) {
    assert.match(kustomization, new RegExp(resource.replaceAll('.', '\\.')));
  }
});
