import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import test from "node:test";

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), "..", "..")]) {
    if (existsSync(resolve(candidate, "remote/argocd/observability/prometheus.configmap.yaml"))) {
      return candidate;
    }
  }
  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), "utf8");
}

function scrapeJob(config: string, job: string): string {
  const marker = `      - job_name: ${job}`;
  const start = config.indexOf(marker);
  assert.notEqual(start, -1, `missing Prometheus job ${job}`);
  const next = config.indexOf("\n      - job_name: ", start + marker.length);
  return next === -1 ? config.slice(start) : config.slice(start, next);
}

function assertDirectScrape(config: string, job: string, target: string): void {
  const block = scrapeJob(config, job);
  assert.match(block, /metrics_path:\s*\/metrics/);
  assert.ok(block.includes(`- ${target}`), `${job} must scrape ${target}`);
}

function assertPodScrapeAnnotations(
  deployment: string,
  port: number,
): void {
  assert.match(deployment, /prometheus\.io\/scrape:\s*['"]true['"]/);
  assert.match(deployment, /prometheus\.io\/path:\s*['"]\/metrics['"]/);
  assert.match(
    deployment,
    new RegExp(`prometheus\\.io/port:\\s*['\"]${port}['\"]`),
  );
}

test("first-wave application Services are registered for direct Prometheus scraping", async () => {
  const prometheus = await readRepoFile(
    "remote/argocd/observability/prometheus.configmap.yaml",
  );

  assertDirectScrape(
    prometheus,
    "fiducia-backend",
    "fiducia-backend.fiducia.svc.cluster.local:8117",
  );
  assertDirectScrape(
    prometheus,
    "canonical-cloud-web",
    "canonical-cloud-web.canonical-cloud.svc.cluster.local:8081",
  );
  assertDirectScrape(
    prometheus,
    "dd-akrion-web-server-rs",
    "dd-akrion-web-server-rs.default.svc.cluster.local:8127",
  );
  assertDirectScrape(
    prometheus,
    "dd-sonus-auris-site",
    "dd-sonus-auris-site.default.svc.cluster.local:9113",
  );

  assert.match(prometheus, /alert:\s*DDFirstPartyApplicationMetricsTargetDown/);
  assert.match(
    prometheus,
    /up\{job=~"fiducia-backend\|canonical-cloud-web\|dd-akrion-web-server-rs\|dd-sonus-auris-site"\}\s*==\s*0/,
  );
});

test("dynamic Rust deployments advertise the same direct scrape path and port", async () => {
  const fiducia = await readRepoFile(
    "remote/argocd/fiducia/fiducia-backend.deployment.yaml",
  );
  const canonical = await readRepoFile(
    "remote/argocd/canonical-cloud/web.deployment.yaml",
  );
  const akrion = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-akrion-web-server-rs.deployment.yaml",
  );

  assertPodScrapeAnnotations(fiducia, 8117);
  assertPodScrapeAnnotations(canonical, 8081);
  assertPodScrapeAnnotations(akrion, 8127);
});

test("static Sonus site uses a locked-down nginx exporter sidecar", async () => {
  const config = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-sonus-auris-site.configmap.yaml",
  );
  const deployment = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-sonus-auris-site.deployment.yaml",
  );
  const service = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-sonus-auris-site.service.yaml",
  );

  assert.match(config, /location = \/stub_status/);
  assert.match(config, /allow 127\.0\.0\.1;/);
  assert.match(config, /deny all;/);
  assert.match(config, /stub_status;/);

  assert.match(
    deployment,
    /image:\s*docker\.io\/nginx\/nginx-prometheus-exporter:1\.5\.1/,
  );
  assert.match(
    deployment,
    /--nginx\.scrape-uri=http:\/\/127\.0\.0\.1:8080\/stub_status/,
  );
  assert.match(deployment, /name:\s*metrics\n\s*containerPort:\s*9113/);
  assert.match(deployment, /readOnlyRootFilesystem:\s*true/);
  assert.match(deployment, /runAsNonRoot:\s*true/);
  assert.match(deployment, /capabilities:\s*\n\s*drop:\s*\n\s*- ALL/);
  assert.doesNotMatch(deployment, /--nginx\.scrape-uri=http:\/\/dd-sonus-auris-site/);

  assert.match(service, /name:\s*metrics\n\s*port:\s*9113/);
  assert.match(service, /targetPort:\s*metrics/);
});

test("machine-readable inventory routes every discovered project workstream to Linear", async () => {
  const inventory = await readRepoFile(
    "docs/observability/prometheus-app-inventory.yaml",
  );
  for (const issue of [
    "DEN-666",
    "DEN-667",
    "DEN-668",
    "DEN-669",
    "DEN-670",
    "DEN-671",
    "DEN-672",
    "DEN-673",
    "DEN-674",
    "DEN-675",
    "DEN-676",
    "DEN-677",
    "DEN-678",
  ]) {
    assert.ok(inventory.includes(issue), `inventory must include ${issue}`);
  }
  assert.match(inventory, /public_gateway_exposure:\s*forbidden-by-default/);
  assert.match(inventory, /label_policy:\s*bounded-non-sensitive/);
});
