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
  assert.match(collector, /dd-remote-rest-api\.default\.svc\.cluster\.local:8082/);
  assert.match(collector, /dd-gleamlang-server\.default\.svc\.cluster\.local:8081/);
  assert.match(collector, /dd-gleam-lambda-runner\.default\.svc\.cluster\.local:8083/);
  assert.match(collector, /dd-gleam-mcp-server\.default\.svc\.cluster\.local:8090/);
  assert.match(collector, /dd-webrtc-signaling\.default\.svc\.cluster\.local:8095/);
  assert.match(collector, /dd-mdp-optimizer\.default\.svc\.cluster\.local:8096/);
  assert.match(collector, /dd-des-simulator\.default\.svc\.cluster\.local:8099/);
  assert.match(collector, /dd-contract-service\.default\.svc\.cluster\.local:8101/);
  assert.match(collector, /dd-trading-server\.default\.svc\.cluster\.local:8103/);
  assert.match(collector, /dd-ai-ml-pipeline\.ai-ml\.svc\.cluster\.local:8099/);
  assert.match(collector, /dd-web-scraper\.default\.svc\.cluster\.local:8097/);
  assert.match(collector, /dd-build-server\.default\.svc\.cluster\.local:8100/);
  assert.match(collector, /dd-container-pool\.default\.svc\.cluster\.local:8102/);
  assert.match(collector, /dd-akka-ws-server\.default\.svc\.cluster\.local:8086/);
  assert.match(collector, /dd-fsharp-ws-server\.default\.svc\.cluster\.local:8087/);
  assert.match(collector, /dd-spark-pipeline-server\.default\.svc\.cluster\.local:8085/);
  assert.match(collector, /dd-formal-methods-server\.default\.svc\.cluster\.local:8110/);
  assert.match(collector, /dd-agent-worker-broker\.default\.svc\.cluster\.local:8098/);
  assert.match(collector, /dd-remote-auth\.default\.svc\.cluster\.local:8083/);
  assert.match(collector, /dd-billing-server\.default\.svc\.cluster\.local:80/);
  assert.match(collector, /dd-formal-methods-service\.default\.svc\.cluster\.local:8111/);
  assert.match(collector, /dd-lock-loadtest-trigger\.default\.svc\.cluster\.local:8110/);
  assert.match(collector, /job_name:\s*gcs-router/);
  assert.match(collector, /kubernetes_sd_configs:[\s\S]*role:\s*pod/);
  assert.match(collector, /__meta_kubernetes_pod_label_app[\s\S]*regex:\s*gcs-router/);
  assert.match(collector, /dd-nats\.messaging\.svc\.cluster\.local:7777/);
  assert.match(collector, /job_name:\s*dd-promtail/);
  assert.match(collector, /__meta_kubernetes_pod_label_app[\s\S]*regex:\s*dd-promtail/);
  assert.match(collector, /endpoint:\s*dd-tempo\.observability\.svc\.cluster\.local:4317/);
  assert.match(collector, /endpoint:\s*dd-jaeger\.observability\.svc\.cluster\.local:4317/);
});

test("prometheus and loki ingest through the collector and promtail fan-in", async () => {
  const prometheus = await readRepoFile("remote/argocd/observability/prometheus.configmap.yaml");
  const promtail = await readRepoFile("remote/argocd/observability/promtail.configmap.yaml");

  assert.match(prometheus, /dd-otel-collector\.observability\.svc\.cluster\.local:8889/);
  assert.match(
    prometheus,
    /job_name:\s*dd-grafana[\s\S]*metrics_path:\s*\/telemetry\/metrics[\s\S]*dd-grafana\.observability\.svc\.cluster\.local:3000/,
  );
  assert.match(
    prometheus,
    /job_name:\s*dd-loki[\s\S]*metrics_path:\s*\/metrics[\s\S]*dd-loki\.observability\.svc\.cluster\.local:3100/,
  );
  assert.doesNotMatch(prometheus, /job_name:\s*observability-stack/);
  assert.match(prometheus, /dd-gleam-mcp-server\.default\.svc\.cluster\.local:8090/);
  assert.match(prometheus, /dd-agent-worker-broker\.default\.svc\.cluster\.local:8098/);
  assert.match(prometheus, /dd-remote-auth\.default\.svc\.cluster\.local:8083/);
  assert.match(prometheus, /dd-billing-server\.default\.svc\.cluster\.local:80/);
  assert.match(prometheus, /dd-formal-methods-service\.default\.svc\.cluster\.local:8111/);
  assert.match(prometheus, /dd-lock-loadtest-trigger\.default\.svc\.cluster\.local:8110/);
  assert.match(prometheus, /gcs-router\.default\.svc\.cluster\.local:9100/);
  assert.match(promtail, /dd-loki\.observability\.svc\.cluster\.local:3100\/loki\/api\/v1\/push/);
  assert.match(promtail, /external_labels:[\s\S]*cluster:\s*dd-ec2/);
  assert.match(promtail, /__path__:\s*\/var\/log\/containers\/\*\.log/);
  assert.doesNotMatch(promtail, /^\s*kubernetes_sd_configs:/m);
  assert.match(promtail, /source:\s*filename[\s\S]*\/var\/log\/containers/);
  assert.match(promtail, /source:\s*pod[\s\S]*\?P<deployment>/);
  assert.match(promtail, /env:\s*stage[\s\S]*environment:\s*stage/);
  assert.match(promtail, /app:\s*deployment[\s\S]*deployment:/);
  assert.match(promtail, /selector:\s*'\{deployment=~"dd-billing-server\|dd-web-scraper\|dd-browser-test-server"\}'/);
  assert.match(promtail, /env:\s*prod[\s\S]*environment:\s*prod/);
  assert.match(promtail, /labeldrop:[\s\S]*-\s*filename/);
  assert.match(promtail, /- cri:\s*\{\}/);
});

test("websocket comparison services expose prometheus metrics", async () => {
  const akkaRoutes = await readRepoFile(
    "remote/deployments/akka-ws-server/src/main/java/com/oresoftware/dd/akkaws/WsRoutes.java",
  );
  const fsharpProgram = await readRepoFile("remote/deployments/fsharp-ws-server/Program.fs");
  const fsharpRoutes = await readRepoFile("remote/deployments/fsharp-ws-server/WsRoutes.fs");

  assert.match(akkaRoutes, /path\("metrics"/);
  assert.match(akkaRoutes, /dd_akka_ws_async_java_messages_in_total/);
  assert.match(akkaRoutes, /dd_akka_ws_akka_streams_messages_in_total/);
  assert.match(fsharpProgram, /MapGet\("\/metrics"/);
  assert.match(fsharpRoutes, /dd_fsharp_ws_messages_in_total/);
  assert.match(fsharpRoutes, /dd_fsharp_ws_uptime_seconds/);
});

test("supporting runtime services expose prometheus metrics", async () => {
  const agentWorker = await readRepoFile("remote/deployments/agent-worker-broker-rs/src/main.rs");
  const authServer = await readRepoFile("remote/deployments/auth-server-rs/src/main.rs");
  const billingApi = await readRepoFile("remote/deployments/billing-server-rs/src/api/mod.rs");
  const formalMethodsService = await readRepoFile(
    "remote/deployments/formal-methods-service-rs/src/routes/mod.rs",
  );
  const lockLoadtest = await readRepoFile("remote/deployments/live-mutex-loadtest-node/src/server.js");
  const agentWorkerDeployment = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-agent-worker-broker.deployment.yaml",
  );
  const authDeployment = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-remote-auth.deployment.yaml",
  );
  const billingDeployment = await readRepoFile(
    "remote/deployments/billing-server-rs/k8s/ec2/dd-billing-server.deployment.yaml",
  );
  const formalDeployment = await readRepoFile(
    "remote/deployments/formal-methods-service-rs/k8s/ec2/dd-formal-methods-service.deployment.yaml",
  );
  const lockDeployment = await readRepoFile(
    "remote/deployments/live-mutex-loadtest-node/k8s/ec2/dd-lock-loadtest-trigger.deployment.yaml",
  );

  assert.match(agentWorker, /\.route\("\/metrics", get\(metrics\)\)/);
  assert.match(agentWorker, /dd_agent_worker_broker_http_requests_total/);
  assert.match(authServer, /\.route\("\/metrics", get\(metrics\)\)/);
  assert.match(authServer, /dd_remote_auth_http_requests_total/);
  assert.match(billingApi, /\.route\("\/metrics", get\(health::metrics\)\)/);
  assert.match(formalMethodsService, /\.route\("\/metrics", get\(health::metrics\)\)/);
  assert.match(lockLoadtest, /url\.pathname === '\/metrics'/);
  assert.match(lockLoadtest, /dd_lock_loadtest_trigger_runs_started_total/);
  assert.match(agentWorkerDeployment, /dd\.dev\/telemetry-revision:\s*'2026-05-18-observability'/);
  assert.match(authDeployment, /dd\.dev\/telemetry-revision:\s*'2026-05-18-observability'/);
  assert.match(billingDeployment, /dd\.dev\/telemetry-revision:\s*'2026-05-18-observability'/);
  assert.match(formalDeployment, /dd\.dev\/telemetry-revision:\s*'2026-05-18-observability'/);
  assert.match(lockDeployment, /dd\.dev\/telemetry-revision:\s*'2026-05-18-observability'/);
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
  const packageJson = await readRepoFile("remote/deployments/dev-server/package.json");
  const telemetry = await readRepoFile("remote/deployments/dev-server/src/telemetry.ts");
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
  const webHome = await readRepoFile("remote/deployments/web-home-rs/src/main.rs");

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
