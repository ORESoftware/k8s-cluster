import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import test from "node:test";

function repoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), "../..")]) {
    if (
      existsSync(
        resolve(
          candidate,
          "remote/argocd/observability/prometheus.configmap.yaml",
        ),
      )
    ) {
      return candidate;
    }
  }
  throw new Error(`unable to locate repository root from ${process.cwd()}`);
}

const root = repoRoot();
const read = (path: string) => readFile(resolve(root, path), "utf8");

function job(config: string, name: string, indentation: number): string {
  const prefix = " ".repeat(indentation);
  const marker = `${prefix}- job_name: ${name}`;
  const start = config.indexOf(marker);
  assert.notEqual(start, -1, `missing scrape job ${name}`);
  const next = config.indexOf(`\n${prefix}- job_name: `, start + marker.length);
  return next < 0 ? config.slice(start) : config.slice(start, next);
}

const services = [
  {
    app: "dd-fabrication-server",
    repo: "fabrication-server.rs",
    port: 8113,
  },
  {
    app: "dd-daedalus-api-server",
    repo: "daedalus-api-server.rs",
    port: 8114,
  },
  {
    app: "dd-daedalus-web-server",
    repo: "daedalus-web-server.rs",
    port: 8115,
  },
] as const;

test("Daedalus application inventory and tenant boundary stay explicit", async () => {
  const [applications, project, tenant] = await Promise.all([
    read("remote/argocd/apps/daedalus.applications.yaml"),
    read("remote/argocd/projects/daedalus.appproject.yaml"),
    read("remote/argocd/projects/daedalus.tenant.yaml"),
  ]);

  assert.match(tenant, /kind:\s*Namespace[\s\S]*name:\s*daedalus/);
  assert.match(tenant, /name:\s*daedalus-default-deny-ingress/);
  assert.match(project, /clusterResourceWhitelist:\s*\[\]/);
  assert.match(project, /namespace:\s*daedalus/);
  assert.match(applications, /left inert until then/);
  assert.match(applications, /dd\/remote-dev\/daedalus-secrets/);

  for (const service of services) {
    assert.match(applications, new RegExp(`name:\\s*${service.app}`));
    assert.match(
      applications,
      new RegExp(
        `repoURL: git@github\\.com:daedalus-fab/${service.repo.replace(".", "\\.")}\\.git`,
      ),
    );
    assert.match(project, new RegExp(`daedalus-fab/${service.repo.replace(".", "\\.")}\\.git`));
  }

  assert.equal(
    (applications.match(/^kind: Application$/gm) ?? []).length,
    services.length,
  );
  assert.equal(
    (applications.match(/targetRevision:\s*main/g) ?? []).length,
    services.length,
  );
  assert.equal(
    (applications.match(/path:\s*k8s/g) ?? []).length,
    services.length,
  );
  assert.equal(
    (applications.match(/namespace:\s*daedalus/g) ?? []).length,
    services.length,
  );
});

test("Prometheus and the collector agree on every Daedalus service target", async () => {
  const [prometheus, collector] = await Promise.all([
    read("remote/argocd/observability/prometheus.configmap.yaml"),
    read("remote/argocd/observability/otel-collector.configmap.yaml"),
  ]);

  for (const service of services) {
    const target = `${service.app}.daedalus.svc.cluster.local:${service.port}`;
    const prometheusJob = job(prometheus, service.app, 6);
    const collectorJob = job(collector, service.app, 12);
    assert.match(prometheusJob, /metrics_path:\s*\/metrics/);
    assert.match(collectorJob, /metrics_path:\s*\/metrics/);
    assert.ok(prometheusJob.includes(`- ${target}`));
    assert.ok(collectorJob.includes(`- ${target}`));
  }
});

test("replica-local fabrication state has direct ready-pod coverage", async () => {
  const collector = await read(
    "remote/argocd/observability/otel-collector.configmap.yaml",
  );
  const direct = job(collector, "dd-fabrication-server-pods", 12);

  assert.match(direct, /role:\s*pod/);
  assert.match(direct, /names:\s*\n\s*-\s*daedalus/);
  assert.match(direct, /__meta_kubernetes_pod_label_app/);
  assert.match(direct, /regex:\s*dd-fabrication-server/);
  assert.match(direct, /__meta_kubernetes_pod_ready/);
  assert.match(direct, /regex:\s*"true"/);
  assert.doesNotMatch(collector, /job_name:\s*dd-daedalus-api-server-pods/);
  assert.doesNotMatch(collector, /job_name:\s*dd-daedalus-web-server-pods/);
});

test("alerts and dashboards use the Daedalus namespace consistently", async () => {
  const [prometheus, dashboards] = await Promise.all([
    read("remote/argocd/observability/prometheus.configmap.yaml"),
    read("remote/argocd/observability/grafana.dashboards.configmap.yaml"),
  ]);

  assert.match(prometheus, /alert:\s*DDFabricationServerTargetDown/);
  assert.match(prometheus, /alert:\s*DDDaedalusApiServerTargetDown/);
  assert.match(prometheus, /alert:\s*DDDaedalusWebServerTargetDown/);
  assert.match(
    prometheus,
    /sum\(up\{job="dd-fabrication-server-pods"\} == bool 1\) < max\(dd_k8s_deployment_desired_replicas\{namespace="daedalus",deployment="dd-fabrication-server",app="dd-fabrication-server"\}\)/,
  );

  const start = dashboards.indexOf("  fabrication-planner.json: |");
  const end = dashboards.indexOf("  economics-server.json: |", start);
  assert.ok(start >= 0 && end > start);
  const fabricationDashboard = dashboards.slice(start, end);
  assert.match(fabricationDashboard, /namespace=\\"daedalus\\"/);
  assert.doesNotMatch(
    fabricationDashboard,
    /namespace=\\"default\\"[^"\n]*dd-fabrication-server|dd-fabrication-server[^"\n]*namespace=\\"default\\"/,
  );
  const dashboardJson = fabricationDashboard
    .split("\n")
    .slice(1)
    .map((line) => line.replace(/^    /, ""))
    .join("\n")
    .trim();
  assert.doesNotThrow(() => JSON.parse(dashboardJson));
});
