import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { readFile, readdir, stat } from "node:fs/promises";
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

type WorkloadKind = "Deployment" | "StatefulSet" | "DaemonSet";
type WorkloadManifest = {
  kind: WorkloadKind;
  namespace: string;
  name: string;
  app: string;
  relativePath: string;
};

async function listYamlFiles(relativeDir: string): Promise<string[]> {
  const absoluteDir = resolve(repoRoot, relativeDir);
  const entries = await readdir(absoluteDir);
  const files: string[] = [];
  for (const entry of entries) {
    const absolutePath = resolve(absoluteDir, entry);
    const relativePath = `${relativeDir}/${entry}`;
    const entryStat = await stat(absolutePath);
    if (entryStat.isDirectory()) {
      files.push(...(await listYamlFiles(relativePath)));
    } else if (/\.ya?ml$/.test(entry)) {
      files.push(relativePath);
    }
  }
  return files;
}

function parseMetadata(doc: string): {
  name?: string;
  namespace?: string;
  labels: Record<string, string>;
} {
  const metadata: { name?: string; namespace?: string; labels: Record<string, string> } = {
    labels: {},
  };
  let inMetadata = false;
  let inLabels = false;
  for (const line of doc.split(/\r?\n/)) {
    if (/^metadata:\s*$/.test(line)) {
      inMetadata = true;
      inLabels = false;
      continue;
    }
    if (inMetadata && /^\S/.test(line)) break;
    if (!inMetadata) continue;

    const name = line.match(/^\s{2}name:\s*['"]?([^'"\n]+)['"]?\s*$/);
    if (name) metadata.name = name[1];
    const namespace = line.match(/^\s{2}namespace:\s*['"]?([^'"\n]+)['"]?\s*$/);
    if (namespace) metadata.namespace = namespace[1];
    if (/^\s{2}labels:\s*$/.test(line)) {
      inLabels = true;
      continue;
    }
    if (inLabels) {
      if (!/^\s{4}/.test(line)) {
        inLabels = false;
        continue;
      }
      const label = line.match(/^\s{4}([^:]+):\s*['"]?([^'"\n]+)['"]?\s*$/);
      if (label) metadata.labels[label[1].trim()] = label[2].trim();
    }
  }
  return metadata;
}

async function readWorkloadManifestInventory(): Promise<WorkloadManifest[]> {
  const files = [
    ...(await listYamlFiles("remote/argocd")),
    ...(await listYamlFiles("remote/deployments")),
  ];
  const byKey = new Map<string, WorkloadManifest>();
  for (const relativePath of files) {
    const source = await readRepoFile(relativePath);
    for (const doc of source.split(/^---\s*$/m)) {
      const kind = doc.match(/^kind:\s*(Deployment|StatefulSet|DaemonSet)\s*$/m)?.[1] as
        | WorkloadKind
        | undefined;
      if (!kind) continue;
      const metadata = parseMetadata(doc);
      if (!metadata.name) continue;
      const namespace = metadata.namespace ?? "default";
      const app = metadata.labels.app ?? metadata.labels["app.kubernetes.io/name"] ?? metadata.name;
      const key = `${kind}/${namespace}/${metadata.name}`;
      byKey.set(key, {
        kind,
        namespace,
        name: metadata.name,
        app,
        relativePath,
      });
    }
  }
  return [...byKey.values()].sort((a, b) =>
    `${a.kind}/${a.namespace}/${a.name}`.localeCompare(`${b.kind}/${b.namespace}/${b.name}`),
  );
}

function csvValuesFromYamlEnv(source: string, name: string): Set<string> {
  const match = source.match(new RegExp(`name:\\s*${name}[\\s\\S]*?value:\\s*([^\\n]+)`));
  assert.ok(match, `expected ${name} env var`);
  return new Set(
    match[1]
      .trim()
      .split(",")
      .map((item) => item.trim())
      .filter(Boolean),
  );
}

function extractDashboardJson(configMap: string, key: string): Record<string, unknown> {
  const marker = `  ${key}: |\n`;
  const start = configMap.indexOf(marker);
  assert.notEqual(start, -1, `expected dashboard key ${key}`);
  const afterMarker = configMap.slice(start + marker.length);
  const lines: string[] = [];
  for (const line of afterMarker.split("\n")) {
    if (/^  \S/.test(line)) break;
    lines.push(line.startsWith("    ") ? line.slice(4) : line);
  }
  return JSON.parse(lines.join("\n")) as Record<string, unknown>;
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
  assert.match(collector, /dd-webrtc-media\.default\.svc\.cluster\.local:8125/);
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
  assert.match(collector, /dd-spark-pipeline-server\.ai-ml\.svc\.cluster\.local:8085/);
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
  assert.match(prometheus, /dd-webrtc-media\.default\.svc\.cluster\.local:8125/);
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
  assert.match(promtail, /selector:\s*'\{deployment=~"dd-billing-server\|dd-web-scraper\|dd-browser-test-server\|dd-selenium-server\|dd-browser-job-runner"\}'/);
  assert.match(promtail, /env:\s*prod[\s\S]*environment:\s*prod/);
  assert.match(promtail, /labeldrop:[\s\S]*-\s*filename/);
  assert.match(promtail, /- cri:\s*\{\}/);
});

test("resource exporter and Grafana fleet dashboard cover every checked-in workload", async () => {
  const inventory = await readWorkloadManifestInventory();
  const namespaces = new Set(inventory.map((item) => item.namespace));
  const apps = new Set(inventory.map((item) => item.app));
  assert.ok(inventory.length >= 70, "expected broad workload inventory coverage");
  assert.ok(apps.has("dd-dev-server-api"));
  assert.ok(apps.has("dd-promtail"));
  assert.ok(apps.has("gcs-mongodb"));

  const exporterConfig = await readRepoFile(
    "remote/argocd/observability/k8s-resource-exporter.configmap.yaml",
  );
  const exporterDeployment = await readRepoFile(
    "remote/argocd/observability/k8s-resource-exporter.deployment.yaml",
  );
  const exporterRbac = await readRepoFile(
    "remote/argocd/observability/k8s-resource-exporter.rbac.yaml",
  );
  const dashboards = await readRepoFile(
    "remote/argocd/observability/grafana.dashboards.configmap.yaml",
  );

  const watchedNamespaces = csvValuesFromYamlEnv(exporterDeployment, "WATCH_NAMESPACES");
  const watchedApps = csvValuesFromYamlEnv(exporterDeployment, "WATCH_APPS");
  for (const namespace of namespaces) {
    assert.ok(watchedNamespaces.has(namespace), `WATCH_NAMESPACES missing ${namespace}`);
  }
  for (const app of apps) {
    assert.ok(watchedApps.has(app), `WATCH_APPS missing ${app}`);
    assert.match(exporterConfig, new RegExp(`(^|[,"])${app.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}([,"])`));
  }

  assert.match(exporterConfig, /def collect_statefulsets/);
  assert.match(exporterConfig, /def collect_daemonsets/);
  assert.match(exporterConfig, /"dd_k8s_workload_%s_replicas" % state/);
  assert.match(exporterConfig, /"desired":/);
  assert.match(exporterConfig, /"available":/);
  assert.match(exporterRbac, /resources:[\s\S]*-\s*daemonsets[\s\S]*-\s*statefulsets/);

  const dashboard = extractDashboardJson(dashboards, "kubernetes-workload-fleet.json");
  const dashboardText = JSON.stringify(dashboard);
  assert.equal(dashboard.title, "Kubernetes Workload Fleet");
  assert.equal(dashboard.uid, "dd-kubernetes-workload-fleet");
  assert.match(dashboardText, /label_values\(dd_k8s_workload_desired_replicas, workload\)/);
  assert.match(dashboardText, /"repeat":"workload"/);
  assert.match(dashboardText, /dd_k8s_workload_unavailable_replicas/);
  assert.match(dashboardText, /\{deployment=~\\\"\$\{workload:regex\}\\\"\}/);
  assert.match(dashboardText, /\{log_schema=\\\"dd\.log\.v1\\\",deployment=~\\\"\$\{workload:regex\}\\\"/);
});

test("promtail parses the shared structured stdio envelope without high-cardinality labels", async () => {
  const promtail = await readRepoFile("remote/argocd/observability/promtail.configmap.yaml");

  assert.match(promtail, /json:[\s\S]*dd_log_schema:\s*schema/);
  assert.match(promtail, /json:[\s\S]*dd_log_severity:\s*severity_text/);
  assert.match(promtail, /json:[\s\S]*dd_log_service:\s*resource_service_name/);
  assert.match(promtail, /labels:[\s\S]*log_schema:\s*dd_log_schema/);
  assert.match(promtail, /labels:[\s\S]*severity:\s*dd_log_severity/);
  assert.match(promtail, /labels:[\s\S]*log_service:\s*dd_log_service/);
  assert.doesNotMatch(promtail, /request[_-]?id:\s*dd_log/i);
  assert.doesNotMatch(promtail, /task[_-]?id:\s*dd_log/i);
  assert.doesNotMatch(promtail, /thread[_-]?id:\s*dd_log/i);
  assert.doesNotMatch(promtail, /trace[_-]?id:\s*dd_log/i);
  assert.doesNotMatch(promtail, /span[_-]?id:\s*dd_log/i);
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
  const pnpmLock = await readRepoFile("remote/deployments/dev-server/pnpm-lock.yaml");
  const telemetry = await readRepoFile("remote/deployments/dev-server/src/telemetry.ts");
  const deployment = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-dev-server-home.deployment.yaml",
  );

  assert.doesNotMatch(packageJson, /@opentelemetry\/auto/);
  assert.doesNotMatch(packageJson, /@opentelemetry\/instrumentation/);
  assert.doesNotMatch(pnpmLock, /@opentelemetry\/auto-instrumentations-node/);
  assert.doesNotMatch(pnpmLock, /@opentelemetry\/instrumentation-(http|fetch|express|fastify)/);
  assert.doesNotMatch(pnpmLock, /require-in-the-middle|shimmer/);
  assert.match(telemetry, /resourceSpans/);
  assert.match(deployment, /OTEL_EXPORTER_OTLP_ENDPOINT/);
  assert.match(deployment, /dd-otel-collector\.observability\.svc\.cluster\.local:4318/);
});

test("queue consumer emits structured critical runtime telemetry", async () => {
  const source = await readRepoFile("remote/deployments/queue-consumer-rs/src/main.rs");
  const readme = await readRepoFile("remote/deployments/queue-consumer-rs/readme.md");
  const deployment = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-remote-queue-consumer.deployment.yaml",
  );
  const runtimeEventsSchema = await readRepoFile(
    "remote/libs/nats/subject-defs/schema/runtime-events.schema.json",
  );
  const generatedRustSubjects = await readRepoFile(
    "remote/libs/nats/subject-defs/generated/rust/src/lib.rs",
  );

  assert.match(runtimeEventsSchema, /"name": "RuntimeCriticalEvents"/);
  assert.match(runtimeEventsSchema, /"subject": "dd\.remote\.events\.critical"/);
  assert.match(runtimeEventsSchema, /"stream": "DD_REMOTE_CRITICAL_EVENTS"/);
  assert.match(runtimeEventsSchema, /"name": "DD_REMOTE_CRITICAL_EVENTS"/);
  assert.match(generatedRustSubjects, /RUNTIME_CRITICAL_EVENTS_SUBJECT/);
  assert.match(generatedRustSubjects, /DD_REMOTE_CRITICAL_EVENTS_STREAM_NAME/);
  assert.match(source, /RUNTIME_CRITICAL_EVENTS_SUBJECT/);
  assert.match(source, /DD_REMOTE_CRITICAL_EVENTS_STREAM_NAME/);
  assert.match(source, /NATS_CRITICAL_EVENT_SUBJECT/);
  assert.match(source, /NATS_CRITICAL_EVENT_STREAM/);
  assert.match(source, /NATS_CRITICAL_EVENT_CONSUMER/);
  assert.match(source, /QUEUE_CONSUMER_CRITICAL_EVENT_LOGGER/);
  assert.match(source, /structured_log_record/);
  assert.match(source, /"schema": LOG_SCHEMA/);
  assert.match(source, /"type": "runtime-critical-event"/);
  assert.match(source, /publish_runtime_critical_event/);
  assert.match(source, /run_critical_event_logger/);
  assert.match(source, /runtime-critical-event-received/);
  assert.match(source, /critical-event-ack-failed/);
  assert.match(source, /invalid-queue-task-message/);
  assert.match(source, /queue-task-ack-failed/);
  assert.doesNotMatch(source, /queue task ack failed:/);
  assert.match(readme, /NATS_CRITICAL_EVENT_SUBJECT/);
  assert.match(readme, /QUEUE_CONSUMER_CRITICAL_EVENT_LOGGER/);
  assert.match(readme, /dd\.remote\.events\.critical/);
  assert.match(deployment, /name:\s*NATS_CRITICAL_EVENT_SUBJECT[\s\S]*value:\s*dd\.remote\.events\.critical/);
  assert.match(deployment, /name:\s*NATS_CRITICAL_EVENT_STREAM[\s\S]*value:\s*DD_REMOTE_CRITICAL_EVENTS/);
  assert.match(deployment, /name:\s*QUEUE_CONSUMER_CRITICAL_EVENT_LOGGER[\s\S]*value:\s*'true'/);
});

test("repo observability contract forbids monkey patching and standardizes stdio", async () => {
  const agents = await readRepoFile("AGENTS.md");
  const contract = await readRepoFile("docs/observability-stdio-contract.md");

  assert.match(agents, /Do not\s+monkey-patch Node\.js, Erlang, Rust, Java/);
  assert.match(agents, /process\.emit\("info", payload\)/);
  assert.match(agents, /process\.on\(\.\.\.\)/);
  assert.doesNotMatch(agents, /Auth:\s+\S+/);
  assert.match(contract, /"schema": "dd\.log\.v1"/);
  assert.match(contract, /time_unix_nano/);
  assert.match(contract, /severity_text/);
  assert.match(contract, /resource_service_name/);
  assert.match(contract, /process\.on\("warning"/);
  assert.match(contract, /process\.on\("info"/);
  assert.match(contract, /Request ids, task ids, thread ids, user ids, trace ids, span ids/);
  assert.match(contract, /OTLP logs are optional/);
  assert.match(contract, /dd\.remote\.events\.critical/);
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
