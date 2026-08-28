import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import test from "node:test";

const FIDUCIA_RELEASE = "957618c8ff7ca746519889573443b5e9e68dde19";
const AKRION_RELEASE = "1bd5dc5a050ce05f9a495e08038cc02e9647e092";
const CANONICAL_RELEASE = "488791e252c2ac94de78e1cd4d71165b53c2a2c3";
const CANONICAL_WEB_PACKAGE = "ghcr.io/canonical-cloud/canonical-web-server";
const CANONICAL_REVOKER_PACKAGE =
  "ghcr.io/canonical-cloud/canonical-session-revoker";
const CANONICAL_WEB_DIGEST =
  "sha256:5e58011c4bf98e9567cf1cf8c901fc71ddb83dc20a4696f17e378e10b050871a";
const CANONICAL_REVOKER_DIGEST =
  "sha256:cb675d56785093ef8a944ed5af5bd5d2387eb052d5e914ebe956cefe09361d5b";
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
  assert.ok(
    block.includes(
      `        static_configs:\n          - targets:\n              - ${target}`,
    ),
    `${job} must place ${target} under static_configs with canonical indentation`,
  );
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
  assertTarget(
    prometheus,
    "usacc-rest-api-backend-rs",
    "usacc-rest-api-backend-rs.default.svc.cluster.local:8121",
  );

  assert.match(prometheus, /alert:\s*DDFirstPartyApplicationMetricsTargetDown/);
  assert.match(
    prometheus,
    /up\{job=~"fiducia-backend\|canonical-cloud-web\|dd-akrion-web-server-rs\|dd-sonus-auris-site\|usacc-rest-api-backend-rs"\}\s*==\s*0/,
  );
  assert.match(prometheus, /alert:\s*DDSonusAurisNginxExporterScrapeFailed/);
  assert.match(prometheus, /nginx_up\{job="dd-sonus-auris-site"\}\s*==\s*0/);
});

test("Shared Auth endpoints are private, scraped, and alerted", async () => {
  const prometheus = await read("remote/argocd/observability/prometheus.configmap.yaml");
  const gateway = await read(
    "remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml",
  );
  const rollout = await read(
    "docs/observability/prometheus-application-rollout.yaml",
  );

  assertTarget(
    prometheus,
    "dd-shared-auth",
    "dd-shared-auth.shared-auth.svc.cluster.local:8120",
  );
  assertTarget(
    prometheus,
    "dd-shared-auth-nats-bridge",
    "dd-shared-auth-nats-bridge.shared-auth.svc.cluster.local:8121",
  );
  for (const alert of [
    "SharedAuthServerTargetMissing",
    "SharedAuthBridgeTargetMissing",
    "SharedAuthMetricsTargetDown",
    "SharedAuthVerificationFailuresHigh",
    "SharedAuthBridgePublishFailuresHigh",
    "SharedAuthBridgeDeliveryFailuresHigh",
  ]) {
    assert.ok(
      prometheus.includes(`          - alert: ${alert}\n            expr:`),
      `missing canonically nested alert ${alert}`,
    );
  }
  assert.match(
    prometheus,
    /up\{job=~"dd-shared-auth\|dd-shared-auth-nats-bridge"\}\s*==\s*0/,
  );
  assert.match(
    prometheus,
    /increase\(shared_auth_verify_failures_total\[10m\]\)\s*>\s*100/,
  );
  assert.match(
    prometheus,
    /increase\(shared_auth_bridge_publishes_total\{outcome="failed"\}\[10m\]\)\)\s*>\s*10/,
  );
  assert.match(
    prometheus,
    /increase\(shared_auth_bridge_deliveries_total\{outcome="failed"\}\[10m\]\)\)\s*>\s*5/,
  );
  assert.doesNotMatch(
    prometheus,
    /shared_auth_bridge_(?:publishes|deliveries)_total\{[^}]*subject=/,
  );

  const metricsDeny = gateway.indexOf("location = /shared-auth/metrics");
  const publicProxy = gateway.indexOf("location /shared-auth/");
  assert.ok(metricsDeny >= 0 && metricsDeny < publicProxy);
  assert.match(
    gateway.slice(metricsDeny, publicProxy),
    /location = \/shared-auth\/metrics \{\s*return 404;/,
  );
  assert.doesNotMatch(gateway, /dd-shared-auth-nats-bridge\.shared-auth/);

  assert.ok(rollout.includes("project: github.com/shared-auth"));
  assert.ok(rollout.includes("expected_replica_targets: 2"));
  assert.ok(rollout.includes("source_revision: 821386ba8b167571b27d91012c665ffcb724ae46"));
  assert.ok(rollout.includes("source_revision: d03316944e33fd59ee90a4acaa4fee67cae71d61"));
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

test("Canonical pins attested monorepo-owned registry digests", async () => {
  const web = await read("remote/argocd/canonical-cloud/web.deployment.yaml");
  const revoker = await read("remote/argocd/canonical-cloud/revoker.deployment.yaml");
  const service = await read("remote/argocd/canonical-cloud/web.service.yaml");

  const webImage = `${CANONICAL_WEB_PACKAGE}@${CANONICAL_WEB_DIGEST}`;
  const revokerImage = `${CANONICAL_REVOKER_PACKAGE}@${CANONICAL_REVOKER_DIGEST}`;
  assert.ok(web.includes(`image: ${webImage}`));
  assert.ok(revoker.includes(`image: ${revokerImage}`));
  assert.ok(web.includes(`canonical.cloud/release-sha: "${CANONICAL_RELEASE}"`));
  assert.ok(revoker.includes(`canonical.cloud/release-sha: "${CANONICAL_RELEASE}"`));
  assert.doesNotMatch(web, /ghcr\.io\/canonical-cloud\/canonical-web-server:/);
  assert.doesNotMatch(revoker, /ghcr\.io\/canonical-cloud\/canonical-session-revoker:/);
  assert.doesNotMatch(web, /canonical-web-server-rs/);
  assert.doesNotMatch(revoker, /canonical-web-server-rs/);
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
  const usaccService = await read(
    "remote/argocd/dd-next-runtime/usacc-rest-api-backend-rs.service.yaml",
  );
  const usaccPolicy = await read(
    "remote/argocd/dd-next-runtime/usacc-rest-api-backend-rs.networkpolicy.yaml",
  );

  assert.match(prometheus, /app:\s*dd-prometheus/);
  assertPrometheusIngress(fiducia, 8117);
  assertPrometheusIngress(canonical, 8081);
  assertPrometheusIngress(akrion, 8127);
  assertPrometheusIngress(sonus.slice(sonus.indexOf("kind: NetworkPolicy")), 9113);
  assertScrapeAnnotations(usaccService, 8121);
  assertPrometheusIngress(usaccPolicy, 8121);
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
  assert.match(gateway, /location = \/canonical\/metrics \{\s*return 404;/);
  assert.match(gateway, /location \^~ \/canonical\/metrics\/ \{\s*return 404;/);
  const canonicalMetricsDeny = gateway.indexOf('location = /canonical/metrics');
  const canonicalMetricsPrefixDeny = gateway.indexOf('location ^~ /canonical/metrics/');
  const canonicalProxy = gateway.indexOf('location /canonical/');
  assert.ok(canonicalMetricsDeny >= 0 && canonicalMetricsDeny < canonicalProxy);
  assert.ok(canonicalMetricsPrefixDeny >= 0 && canonicalMetricsPrefixDeny < canonicalProxy);
  assert.match(gateway, /location \^~ \/akrion-sim\/metrics \{\s*return 404;/);
  assert.doesNotMatch(
    gateway,
    /dd-sonus-auris-site\.default\.svc\.cluster\.local:9113/,
  );
});
