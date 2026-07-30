import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import test from "node:test";

function repoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), "../..")]) {
    if (existsSync(resolve(candidate, "remote/argocd/observability/prometheus.deployment.yaml"))) {
      return candidate;
    }
  }
  throw new Error(`unable to locate repository root from ${process.cwd()}`);
}

const root = repoRoot();
const read = (path: string) => readFile(resolve(root, path), "utf8");

interface DiscoveryPod {
  name: string;
  ip?: string;
  ready: boolean;
  terminating?: boolean;
  annotations: Record<string, string>;
}

function eligibleTargets(pods: DiscoveryPod[]): string[] {
  return pods
    .filter((pod) => pod.annotations["prometheus.io/scrape"] === "true")
    .filter((pod) => pod.ready && !pod.terminating && Boolean(pod.ip))
    .filter((pod) => /^\/[\S]*$/.test(pod.annotations["prometheus.io/path"] ?? ""))
    .filter((pod) => {
      const port = Number(pod.annotations["prometheus.io/port"]);
      return Number.isInteger(port) && port >= 1 && port <= 65_535;
    })
    .map((pod) => `${pod.ip}:${pod.annotations["prometheus.io/port"]}`);
}

test("Prometheus discovery identity is read-only and intentionally narrow", async () => {
  const rbac = await read("remote/argocd/observability/prometheus.rbac.yaml");
  const kustomization = await read("remote/argocd/observability/kustomization.yaml");

  assert.match(rbac, /kind:\s*ServiceAccount[\s\S]*name:\s*dd-prometheus/);
  assert.match(rbac, /automountServiceAccountToken:\s*false/);
  assert.match(rbac, /kind:\s*ClusterRole[\s\S]*name:\s*dd-prometheus-discovery/);
  for (const resource of ["namespaces", "pods", "services", "endpoints", "endpointslices"]) {
    assert.match(rbac, new RegExp(`- ${resource}\\b`));
  }
  for (const verb of ["get", "list", "watch"]) {
    assert.match(rbac, new RegExp(`- ${verb}\\b`));
  }
  assert.doesNotMatch(rbac, /-\s*(?:create|update|patch|delete|deletecollection|impersonate)\b/);
  assert.doesNotMatch(rbac, /-\s*(?:secrets|configmaps|nodes|nodes\/proxy)\b/);
  assert.match(
    rbac,
    /kind:\s*ClusterRoleBinding[\s\S]*kind:\s*ServiceAccount[\s\S]*name:\s*dd-prometheus[\s\S]*namespace:\s*observability/,
  );
  assert.match(kustomization, /- prometheus\.rbac\.yaml/);
});

test("Prometheus uses a bounded projected token without implicit automount", async () => {
  const deployment = await read("remote/argocd/observability/prometheus.deployment.yaml");

  assert.match(deployment, /serviceAccountName:\s*dd-prometheus/);
  assert.match(deployment, /automountServiceAccountToken:\s*false/);
  assert.match(
    deployment,
    /name:\s*kubernetes-api-access[\s\S]*mountPath:\s*\/var\/run\/secrets\/prometheus-discovery[\s\S]*readOnly:\s*true/,
  );
  assert.match(deployment, /serviceAccountToken:/);
  assert.doesNotMatch(
    deployment,
    /^\s+audience:\s*/m,
    "use the API server's configured audience unless the cluster contract names a custom audience",
  );
  assert.match(deployment, /expirationSeconds:\s*3600/);
  assert.match(deployment, /path:\s*token/);
  assert.match(deployment, /name:\s*kube-root-ca\.crt/);
  assert.match(deployment, /path:\s*ca\.crt/);
  assert.doesNotMatch(deployment, /\/var\/run\/secrets\/kubernetes\.io\/serviceaccount/);
});

test("the versioned contract keeps static targets until duplicate and scale-zero gates pass", async () => {
  const contract = await read("docs/observability/prometheus-pod-discovery-contract-v1.yaml");
  const inventory = await read("docs/observability/prometheus-application-rollout.yaml");
  const prometheus = await read("remote/argocd/observability/prometheus.configmap.yaml");

  assert.match(contract, /schema_version:\s*1/);
  assert.match(contract, /contract:\s*dd\.dev\/prometheus-pod-discovery/);
  assert.match(contract, /state:\s*prerequisites-only/);
  assert.match(contract, /prometheus\.io\/scrape:\s*"true"/);
  assert.match(contract, /ready_only:\s*true/);
  assert.match(contract, /terminating_pods_eligible:\s*false/);
  assert.match(contract, /audience_source:\s*api-server-default/);
  assert.match(contract, /custom_audience_configured:\s*false/);
  assert.match(contract, /enabled:\s*false/);
  assert.match(contract, /zero overlap with static application jobs/);
  assert.match(contract, /scaled-to-zero workloads do not fire target-missing alerts/);
  assert.match(contract, /scaled_to_zero_alert_policy:\s*suppress_target_missing/);

  assert.match(inventory, /schema_version:\s*2/);
  assert.match(inventory, /phase:\s*prerequisites/);
  assert.match(inventory, /planned_job:\s*first-party-pods-v1/);
  assert.match(inventory, /duplicate_target_gate:\s*required-before-cutover/);
  assert.equal(
    [...inventory.matchAll(/expected_replica_targets:\s*1/g)].length,
    5,
    "each first-wave workload must record its current expected target cardinality",
  );

  assert.doesNotMatch(prometheus, /kubernetes_sd_configs:/);
  for (const job of [
    "fiducia-backend",
    "canonical-cloud-web",
    "dd-akrion-web-server-rs",
    "dd-sonus-auris-site",
    "usacc-rest-api-backend-rs",
  ]) {
    assert.match(prometheus, new RegExp(`job_name: ${job}\\b`));
  }
});

test("the v1 eligibility model yields one target per ready pod and none at scale zero", () => {
  const annotations = {
    "prometheus.io/scrape": "true",
    "prometheus.io/path": "/metrics",
    "prometheus.io/port": "8081",
  };
  const targets = eligibleTargets([
    { name: "web-0", ip: "10.0.0.10", ready: true, annotations },
    { name: "web-1", ip: "10.0.0.11", ready: true, annotations },
    { name: "web-2", ip: "10.0.0.12", ready: false, annotations },
    { name: "web-3", ip: "10.0.0.13", ready: true, terminating: true, annotations },
  ]);

  assert.deepEqual(targets, ["10.0.0.10:8081", "10.0.0.11:8081"]);
  assert.deepEqual(eligibleTargets([]), []);
});
