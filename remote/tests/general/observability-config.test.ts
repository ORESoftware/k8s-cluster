import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import test from "node:test";

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), "..", "..")]) {
    if (existsSync(resolve(candidate, "remote/argocd/observability/kustomization.yaml"))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), "utf8");
}

test("observability kustomization installs collector, metrics, logs, traces, and UI", async () => {
  const kustomization = await readRepoFile("remote/argocd/observability/kustomization.yaml");

  assert.match(kustomization, /otel-collector\.deployment\.yaml/);
  assert.match(kustomization, /prometheus\.deployment\.yaml/);
  assert.match(kustomization, /grafana\.deployment\.yaml/);
  assert.match(kustomization, /loki\.deployment\.yaml/);
  assert.match(kustomization, /promtail\.daemonset\.yaml/);
  assert.match(kustomization, /tempo\.deployment\.yaml/);
  assert.match(kustomization, /jaeger\.deployment\.yaml/);
});

test("otel collector scrapes all remote runtimes and exports traces", async () => {
  const collector = await readRepoFile(
    "remote/argocd/observability/otel-collector.configmap.yaml",
  );

  assert.match(collector, /dd-dev-server-api\.default\.svc\.cluster\.local:8080/);
  assert.match(collector, /dd-remote-web-home\.default\.svc\.cluster\.local:8080/);
  assert.match(collector, /dd-gleamlang-server\.default\.svc\.cluster\.local:8081/);
  assert.match(collector, /dd-gleam-lambda-runner\.default\.svc\.cluster\.local:8083/);
  assert.match(collector, /dd-gleam-mcp-server\.default\.svc\.cluster\.local:8090/);
  assert.match(collector, /endpoint:\s*dd-tempo\.observability\.svc\.cluster\.local:4317/);
  assert.match(collector, /endpoint:\s*dd-jaeger\.observability\.svc\.cluster\.local:4317/);
});

test("public gateway exposes grafana under /telemetry", async () => {
  const gateway = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml",
  );

  assert.match(gateway, /location = \/telemetry/);
  assert.match(gateway, /return 302 \/telemetry\//);
  assert.match(gateway, /location\s+\/telemetry\//);
  assert.match(gateway, /X-Forwarded-Prefix \/telemetry/);
  assert.match(gateway, /dd-grafana\.observability\.svc\.cluster\.local:3000/);
});

test("node worker uses explicit OTLP endpoint without opentelemetry sdk deps", async () => {
  const packageJson = await readRepoFile("remote/dev-server/package.json");
  const telemetry = await readRepoFile("remote/dev-server/src/telemetry.ts");
  const deployment = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-dev-server-home.deployment.yaml",
  );

  assert.doesNotMatch(packageJson, /@opentelemetry\/auto/);
  assert.doesNotMatch(packageJson, /@opentelemetry\/instrumentation/);
  assert.match(telemetry, /resourceSpans/);
  assert.match(deployment, /OTEL_EXPORTER_OTLP_ENDPOINT/);
  assert.match(deployment, /dd-otel-collector\.observability\.svc\.cluster\.local:4318/);
});

test("rust public web telemetry keeps aligned service metadata", async () => {
  const deployment = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-remote-web-home.deployment.yaml",
  );
  const webHome = await readRepoFile("remote/web-home-rs/src/main.rs");

  assert.match(deployment, /dd\.dev\/telemetry-revision/);
  assert.match(webHome, /service:\s*"dd-remote-web-home"\.to_string\(\)/);
  assert.match(webHome, /with_label_values\(&\[\s*"dd-remote-web-home"/);
});

test("grafana dashboard includes the Gleam MCP runtime metrics", async () => {
  const dashboard = await readRepoFile(
    "remote/argocd/observability/grafana.dashboards.configmap.yaml",
  );

  assert.match(dashboard, /Gleam MCP Runtime/);
  assert.match(dashboard, /dd_gleam_mcp_http_requests_total/);
  assert.match(dashboard, /dd_gleam_mcp_rpc_requests_total/);
});
