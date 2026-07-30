import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import test from "node:test";

const FIDUCIA_RELEASE = "957618c8ff7ca746519889573443b5e9e68dde19";
const AKRION_RELEASE = "1bd5dc5a050ce05f9a495e08038cc02e9647e092";
const CANONICAL_RELEASE = "e09bb95160aaf95a836e810eb20b65e74f6317a6";
const CANONICAL_PACKAGE = "ghcr.io/canonical-cloud/canonical-web-server-rs";
const SONUS_EXPORTER =
  "docker.io/nginx/nginx-prometheus-exporter:1.5.1@sha256:9f6d963bb2b19d706d401cc3e2c3ea8de2f1c471b96a2156ca45e76f650b1625";

function repoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), "../..")]) {
    if (existsSync(resolve(candidate, "remote/argocd/observability/prometheus.configmap.yaml"))) {
      return candidate;
    }
  }
  throw new Error(`unable to locate repository root from ${process.cwd()}`);
}

const root = repoRoot();
const read = (path: string) => readFile(resolve(root, path), "utf8");

function scrapeJob(config: string, name: string): string {
  const marker = `      - job_name: ${name}`;
  const start = config.indexOf(marker);
  assert.notEqual(start, -1, `missing Prometheus job ${name}`);
  const next = config.indexOf("\n      - job_name: ", start + marker.length);
  return next < 0 ? config.slice(start) : config.slice(start, next);
}

function assertTarget(config: string, job: string, target: string): void {
  const block = scrapeJob(config, job);
  assert.match(block, /metrics_path:\s*\/metrics/);
  assert.ok(block.includes(`- ${target}`), `${job} must scrape ${target}`);
}

function assertScrapeAnnotations(resource: string, port: number): void {
  assert.match(resource, /prometheus\.io\/scrape:\s*['"]true['"]/);
  assert.match(resource, /prometheus\.io\/path:\s*['"]\/metrics['"]/);
  assert.match(
    resource,
    new RegExp(`prometheus\\.io/port:\\s*['\"]${port}['\"]`),
  );
}

function assertPrometheusIngress(policy: string, port: number): void {
  assert.match(policy, /kubernetes\.io\/metadata\.name:\s*observability/);
  assert.match(policy, /(?:app:\s*dd-prometheus|-\s*dd-prometheus)/);
  assert.match(policy, new RegExp(`port:\\s*${port}(?:\\s|$)`));
}

test("central Prometheus scrapes each first-wave application every 15 seconds", async () => {
  const prometheus = await read("remote/argocd/observability/prometheus.configmap.yaml");

  assert.match(prometheus, /scrape_interval:\s*15s/);
  assertTarget(
    prometheus,
    "fiducia-backend",
    "fiducia-backend.fiducia.svc.cluster.local:8117",
  );
  assertTarget(
    prometheus,
    "canonical-cloud-web",
    "canonical-cloud-web.canonical-cloud.svc.cluster.local:8081",
  );
  assertTarget(
    prometheus,
    "dd-akrion-web-server-rs",
    "dd-akrion-web-server-rs.default.svc.cluster.local:8127",
  );
  assertTarget(
    prometheus,
    "dd-sonus-auris-site",
    "dd-sonus-auris-site.default.svc.cluster.local:9113",
  );

  assert.match(prometheus, /alert:\s*DDFirstPartyApplicationMetricsTargetDown/);
  assert.match(
    prometheus,
    /up\{job=~"fiducia-backend\|canonical-cloud-web\|dd-akrion-web-server-rs\|dd-sonus-auris-site"\}\s*==\s*0/,
  );
  assert.match(prometheus, /alert:\s*DDSonusAurisNginxExporterScrapeFailed/);
  assert.match(prometheus, /nginx_up\{job="dd-sonus-auris-site"\}\s*==\s*0/);
});

test("Fiducia and Akrion build the reviewed merged revisions", async () => {
  const fiducia = await read("remote/argocd/fiducia/fiducia-backend.deployment.yaml");
  const akrion = await read(
    "remote/argocd/dd-next-runtime/dd-akrion-web-server-rs.deployment.yaml",
  );

  assert.ok(fiducia.includes(FIDUCIA_RELEASE));
  assert.match(fiducia, /git -C fiducia-customer\.rs fetch --depth 1 origin/);
  assert.match(fiducia, /git -C fiducia-customer\.rs switch --detach FETCH_HEAD/);
  assertScrapeAnnotations(fiducia, 8117);

  assert.ok(akrion.includes(AKRION_RELEASE));
  assert.match(
    akrion,
    /git -C "\$app_dir" "\$\{git_auth_args\[@\]\}" fetch --depth 1 origin/,
  );
  assert.match(akrion, /actual_ref=.*rev-parse HEAD/);
  assertScrapeAnnotations(akrion, 8127);
  assert.doesNotMatch(akrion, /AKRION_WEB_GIT_REF\n\s*value:\s*dev/);
});

test("Canonical pins matching immutable component tags in one repository-owned package", async () => {
  const web = await read("remote/argocd/canonical-cloud/web.deployment.yaml");
  const revoker = await read("remote/argocd/canonical-cloud/revoker.deployment.yaml");
  const service = await read("remote/argocd/canonical-cloud/web.service.yaml");

  const webImage = `${CANONICAL_PACKAGE}:web-${CANONICAL_RELEASE}`;
  const revokerImage = `${CANONICAL_PACKAGE}:revoker-${CANONICAL_RELEASE}`;
  assert.ok(web.includes(`image: ${webImage}`));
  assert.ok(revoker.includes(`image: ${revokerImage}`));
  assert.ok(web.includes(`canonical.cloud/release-sha: "${CANONICAL_RELEASE}"`));
  assert.ok(revoker.includes(`canonical.cloud/release-sha: "${CANONICAL_RELEASE}"`));
  assert.doesNotMatch(web, /ghcr\.io\/canonical-cloud\/canonical-web-server:/);
  assert.doesNotMatch(revoker, /ghcr\.io\/canonical-cloud\/canonical-session-revoker:/);
  assertScrapeAnnotations(web, 8081);
  assertScrapeAnnotations(service, 8081);
});

test("NetworkPolicies admit only the intended Prometheus workload on scrape ports", async () => {
  const prometheus = await read("remote/argocd/observability/prometheus.deployment.yaml");
  const fiducia = await read("remote/argocd/fiducia/fiducia-backend.networkpolicy.yaml");
  const canonical = await read("remote/argocd/canonical-cloud/web.networkpolicy.yaml");
  const akrion = await read(
    "remote/argocd/dd-next-runtime/dd-akrion-web-server-rs.networkpolicy.yaml",
  );
  const sonus = await read(
    "remote/argocd/dd-next-runtime/dd-sonus-auris-site.service.yaml",
  );

  assert.match(prometheus, /app:\s*dd-prometheus/);
  assertPrometheusIngress(fiducia, 8117);
  assertPrometheusIngress(canonical, 8081);
  assertPrometheusIngress(akrion, 8127);
  assertPrometheusIngress(sonus.slice(sonus.indexOf("kind: NetworkPolicy")), 9113);
});

test("Sonus exports nginx status through a hardened digest-pinned sidecar", async () => {
  const config = await read(
    "remote/argocd/dd-next-runtime/dd-sonus-auris-site.configmap.yaml",
  );
  const deployment = await read(
    "remote/argocd/dd-next-runtime/dd-sonus-auris-site.deployment.yaml",
  );
  const service = await read(
    "remote/argocd/dd-next-runtime/dd-sonus-auris-site.service.yaml",
  );

  assert.match(config, /location = \/stub_status/);
  assert.match(config, /allow 127\.0\.0\.1;/);
  assert.match(config, /deny all;/);
  assert.match(config, /stub_status;/);
  assert.ok(deployment.includes(SONUS_EXPORTER));
  assert.match(
    deployment,
    /--nginx\.scrape-uri=http:\/\/127\.0\.0\.1:8080\/stub_status/,
  );
  assert.match(deployment, /readOnlyRootFilesystem:\s*true/);
  assert.match(deployment, /capabilities:\s*\n\s*drop:\s*\n\s*- ALL/);
  assert.match(service, /name:\s*metrics\n\s*port:\s*9113/);
  assertScrapeAnnotations(deployment, 9113);
  assertScrapeAnnotations(service, 9113);
});

test("application metrics remain off customer-facing gateway routes", async () => {
  const gateway = await read(
    "remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml",
  );

  assert.match(
    gateway,
    /server_name app\.fiducia\.cloud;[\s\S]*?location \^~ \/metrics \{\s*return 404;/,
  );
  assert.match(gateway, /location \^~ \/akrion-sim\/metrics \{\s*return 404;/);
  assert.doesNotMatch(
    gateway,
    /dd-sonus-auris-site\.default\.svc\.cluster\.local:9113/,
  );
});
