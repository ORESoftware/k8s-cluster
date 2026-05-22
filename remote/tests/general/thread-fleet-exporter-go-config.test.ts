import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/deployments/thread-fleet-exporter-go/go.mod'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('dd-thread-fleet-exporter-go is a read-only Prometheus exporter', async () => {
  const goMod = await readRepoFile('remote/deployments/thread-fleet-exporter-go/go.mod');
  const main = await readRepoFile(
    'remote/deployments/thread-fleet-exporter-go/cmd/exporter/main.go',
  );

  assert.match(
    goMod,
    /module github\.com\/ORESoftware\/k8s-cluster\/remote\/deployments\/thread-fleet-exporter-go/,
  );
  assert.match(goMod, /github\.com\/prometheus\/client_golang/);
  assert.match(goMod, /k8s\.io\/client-go/);

  // Exposed metric names form the contract grafana/alerting will hang off of.
  // Tolerate gofmt's alignment whitespace after `Name:`.
  assert.match(main, /Name:\s+"dd_thread_fleet_total"/);
  assert.match(main, /Name:\s+"dd_thread_fleet_replicas_desired_total"/);
  assert.match(main, /Name:\s+"dd_thread_fleet_replicas_ready_total"/);
  assert.match(main, /Name:\s+"dd_thread_fleet_pvcs_total"/);
  assert.match(main, /Name:\s+"dd_thread_fleet_threads"/);
  assert.match(main, /Name:\s+"dd_thread_fleet_scrape_seconds"/);
  assert.match(main, /Name:\s+"dd_thread_fleet_scrape_errors_total"/);

  // Phase taxonomy must match /u/admin/k8s lifecycle.
  assert.match(main, /"active", "starting", "sleeping", "failed", "dead"/);

  // PVC state taxonomy is the standard k8s set + 'unknown' bucket.
  assert.match(main, /"bound", "pending", "lost", "unknown"/);

  // Read-only: only k8s LIST + WATCH calls; no Create/Update/Delete.
  assert.match(main, /\.Deployments\(cfg\.namespace\)\.List/);
  assert.match(main, /\.Pods\(cfg\.namespace\)\.List/);
  assert.match(main, /\.PersistentVolumeClaims\(cfg\.namespace\)\.List/);
  assert.doesNotMatch(main, /\.Create\(/);
  assert.doesNotMatch(main, /\.Update\(/);
  assert.doesNotMatch(main, /\.Delete\(/);

  // /metrics + /healthz endpoints both exist.
  assert.match(main, /mux\.Handle\("\/metrics"/);
  assert.match(main, /mux\.HandleFunc\("\/healthz"/);

  // The doc-vs-impl mismatch must stay fixed: no claim of an
  // unimplemented dd_thread_fleet_age_seconds histogram.
  assert.doesNotMatch(main, /dd_thread_fleet_age_seconds/);
});

test('dd-thread-fleet-exporter manifests are read-only and run as non-root', async () => {
  const rbac = await readRepoFile(
    'remote/deployments/thread-fleet-exporter-go/k8s/ec2/00-rbac.yaml',
  );
  const dep = await readRepoFile(
    'remote/deployments/thread-fleet-exporter-go/k8s/ec2/01-deployment.yaml',
  );
  const svc = await readRepoFile(
    'remote/deployments/thread-fleet-exporter-go/k8s/ec2/02-service.yaml',
  );
  const kustomization = await readRepoFile(
    'remote/deployments/thread-fleet-exporter-go/k8s/ec2/kustomization.yaml',
  );

  // RBAC: read-only only. Allowed verbs are get/list/watch on a small set.
  assert.match(rbac, /name: dd-thread-fleet-exporter/);
  assert.match(rbac, /verbs: \[get, list, watch\]/);
  assert.doesNotMatch(rbac, /\bcreate\b/);
  assert.doesNotMatch(rbac, /\bupdate\b/);
  assert.doesNotMatch(rbac, /\bpatch\b/);
  assert.doesNotMatch(rbac, /\bdelete\b/);
  assert.match(rbac, /resources: \[pods, persistentvolumeclaims\]/);
  assert.match(rbac, /resources: \[deployments\]/);

  // Deployment: secure pod + container securityContext.
  assert.match(dep, /securityContext:\s*\n\s*runAsNonRoot: true/);
  assert.match(dep, /runAsUser: 1000/);
  assert.match(dep, /allowPrivilegeEscalation: false/);
  assert.match(dep, /drop: \[ALL\]/);
  assert.match(dep, /type: RuntimeDefault/);
  assert.match(
    dep,
    /\/home\/ec2-user\/codes\/dd\/dd-next-1\/remote\/deployments\/thread-fleet-exporter-go/,
  );
  assert.match(dep, /--namespace=dd-dev/);
  assert.match(dep, /--label-selector=app\.kubernetes\.io\/component=thread-pod/);

  // Service: ClusterIP, metrics on 9103 only.
  assert.match(svc, /type: ClusterIP/);
  assert.match(svc, /port: 9103/);

  // kustomization aggregates all three files.
  for (const f of ['00-rbac.yaml', '01-deployment.yaml', '02-service.yaml']) {
    assert.match(kustomization, new RegExp(`-\\s+${f.replace('.', '\\.')}`));
  }
});

test('dd-thread-fleet-exporter argocd Application points at this overlay', async () => {
  const app = await readRepoFile(
    'remote/argocd/apps/dd-thread-fleet-exporter.application.yaml',
  );

  assert.match(app, /kind: Application/);
  assert.match(app, /name: dd-thread-fleet-exporter/);
  assert.match(app, /path: remote\/deployments\/thread-fleet-exporter-go\/k8s\/ec2/);
  assert.match(app, /automated:\s*\n\s*prune: true\s*\n\s*selfHeal: true/);
});
