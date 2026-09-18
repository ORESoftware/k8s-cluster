import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
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
  const prometheusDeployment = await readRepoFile(
    "remote/argocd/observability/prometheus.deployment.yaml",
  );

  assert.match(kustomization, /otel-collector\.deployment\.yaml/);
  assert.match(kustomization, /prometheus\.deployment\.yaml/);
  assert.match(kustomization, /grafana\.deployment\.yaml/);
  assert.match(kustomization, /loki\.deployment\.yaml/);
  assert.match(kustomization, /promtail\.daemonset\.yaml/);
  assert.match(kustomization, /tempo\.deployment\.yaml/);
  assert.match(kustomization, /jaeger\.deployment\.yaml/);
  assert.match(prometheusDeployment, /replicas:\s*1/);
  assert.match(prometheusDeployment, /strategy:[\s\S]*type:\s*Recreate/);
});

test("otel collector scrapes all remote runtimes and exports traces", async () => {
  const collector = await readRepoFile(
    "remote/argocd/observability/otel-collector.configmap.yaml",
  );
  const collectorDeployment = await readRepoFile(
    "remote/argocd/observability/otel-collector.deployment.yaml",
  );
  const collectorService = await readRepoFile(
    "remote/argocd/observability/otel-collector.service.yaml",
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
  assert.match(collector, /health_check:[\s\S]*endpoint:\s*0\.0\.0\.0:13133/);
  assert.match(collector, /const_labels:[\s\S]*cluster:\s*dd-ec2/);
  assert.match(collector, /telemetry:[\s\S]*metrics:[\s\S]*address:\s*0\.0\.0\.0:8888/);
  assert.match(collector, /endpoint:\s*dd-tempo\.observability\.svc\.cluster\.local:4317/);
  assert.match(collector, /endpoint:\s*dd-jaeger\.observability\.svc\.cluster\.local:4317/);
  assert.match(collectorDeployment, /name:\s*self-metrics[\s\S]*containerPort:\s*8888/);
  assert.match(collectorDeployment, /name:\s*health[\s\S]*containerPort:\s*13133/);
  assert.match(collectorDeployment, /readinessProbe:[\s\S]*httpGet:[\s\S]*port:\s*13133/);
  assert.match(collectorDeployment, /livenessProbe:[\s\S]*httpGet:[\s\S]*port:\s*13133/);
  assert.match(collectorService, /name:\s*self-metrics[\s\S]*port:\s*8888[\s\S]*targetPort:\s*8888/);
  assert.match(collectorService, /name:\s*health[\s\S]*port:\s*13133[\s\S]*targetPort:\s*13133/);
});

test("prometheus and loki ingest through the collector and promtail fan-in", async () => {
  const prometheus = await readRepoFile("remote/argocd/observability/prometheus.configmap.yaml");
  const promtail = await readRepoFile("remote/argocd/observability/promtail.configmap.yaml");
  const loki = await readRepoFile("remote/argocd/observability/loki.configmap.yaml");
  const lokiDeployment = await readRepoFile("remote/argocd/observability/loki.deployment.yaml");

  assert.match(prometheus, /rule_files:[\s\S]*\/etc\/prometheus\/observability\.rules\.yml/);
  assert.match(prometheus, /job_name:\s*otel-collector-self[\s\S]*dd-otel-collector\.observability\.svc\.cluster\.local:8888/);
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
  assert.match(prometheus, /record:\s*dd:observability:target_up_ratio/);
  assert.match(prometheus, /alert:\s*DDObservabilityTargetDown/);
  assert.match(prometheus, /alert:\s*DDOtelCollectorRejectedTelemetry/);
  assert.match(promtail, /dd-loki\.observability\.svc\.cluster\.local:3100\/loki\/api\/v1\/push/);
  assert.match(promtail, /batchwait:\s*1s/);
  assert.match(promtail, /batchsize:\s*1048576/);
  assert.match(promtail, /backoff_config:[\s\S]*max_retries:\s*10/);
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
  assert.match(loki, /limits_config:[\s\S]*reject_old_samples:\s*true/);
  assert.match(loki, /ingestion_rate_mb:\s*16/);
  assert.match(loki, /max_global_streams_per_user:\s*5000/);
  assert.match(lokiDeployment, /configmap\.reloader\.stakater\.com\/reload:\s*"dd-loki-config"/);
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

  const controlPlaneDashboard = extractDashboardJson(dashboards, "observability-control-plane.json");
  const controlPlaneText = JSON.stringify(controlPlaneDashboard);
  assert.equal(controlPlaneDashboard.title, "Observability Control Plane");
  assert.equal(controlPlaneDashboard.uid, "dd-observability-control-plane");
  assert.match(controlPlaneText, /dd:observability:target_up_ratio/);
  assert.match(controlPlaneText, /otel-collector-self/);
  assert.match(controlPlaneText, /promtail_read_lines_total/);
  assert.match(controlPlaneText, /otelcol_receiver_refused_/);
  assert.match(controlPlaneText, /\{namespace=\\\"observability\\\"\}/);
});

test("standalone observability coverage guardrail passes", async () => {
  const result = spawnSync("node", ["remote/tools/check-observability-coverage.mjs"], {
    cwd: repoRoot,
    encoding: "utf8",
  });

  assert.equal(
    result.status,
    0,
    `observability coverage check failed\nSTDOUT:\n${result.stdout}\nSTDERR:\n${result.stderr}`,
  );
  assert.match(result.stdout, /observability coverage ok/);
  assert.match(result.stdout, /source files avoid common monkey-patching patterns/);
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
  assert.match(billingDeployment, /dd\.dev\/telemetry-revision:\s*'2026-06-05-customer-snapshot-locks'/);
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

test("grafana exposes a dedicated fabrication planner dashboard", async () => {
  const dashboards = await readRepoFile(
    "remote/argocd/observability/grafana.dashboards.configmap.yaml",
  );
  const prometheus = await readRepoFile("remote/argocd/observability/prometheus.configmap.yaml");
  const observabilityReadme = await readRepoFile("remote/argocd/observability/readme.md");
  const gateway = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml",
  );
  const runtimeReadme = await readRepoFile("remote/argocd/dd-next-runtime/readme.md");
  const webHome = await readRepoFile("remote/deployments/web-home-rs/src/main.rs");

  const dashboard = extractDashboardJson(dashboards, "fabrication-planner.json");
  const dashboardText = JSON.stringify(dashboard);

  assert.equal(dashboard.title, "Fabrication Planner");
  assert.equal(dashboard.uid, "dd-fabrication-planner");
  assert.match(dashboardText, /dd_fabrication_server_plan_requests_total/);
  assert.match(dashboardText, /dd_fabrication_server_analysis_requests_total/);
  assert.match(dashboardText, /dd_fabrication_server_failure_boundaries_total/);
  assert.match(dashboardText, /dd_fabrication_server_validation_findings_total/);
  assert.match(dashboardText, /dd_fabrication_server_nats_results_published_total/);
  assert.match(dashboardText, /dd_fabrication_server_mdp_published_total/);
  assert.match(dashboardText, /Generated Programs, Artifacts, Learning Events, and Fetches/);
  assert.match(dashboardText, /dd_fabrication_server_generated_programs_total/);
  assert.match(dashboardText, /dd_fabrication_server_jobs_stored_total/);
  assert.match(dashboardText, /dd_fabrication_server_artifacts_stored_total/);
  assert.match(dashboardText, /dd_fabrication_server_learning_events_stored_total/);
  assert.match(dashboardText, /dd_runtime_config_push_total/);
  assert.match(dashboardText, /dd_k8s_hpa_current_at_max/);
  assert.match(dashboardText, /dd_k8s_deployment_updated_replicas/);
  assert.match(dashboardText, /dd_k8s_deployment_unavailable_replicas/);
  assert.match(dashboardText, /Direct Pod Scrape Coverage/);
  assert.match(dashboardText, /ready direct pod scrapes/);
  assert.match(dashboardText, /scrape coverage gap/);
  assert.match(dashboardText, /dd_k8s_deployment_desired_replicas/);
  assert.match(dashboardText, /Machine Release Readiness and Learning Fanout/);
  assert.match(dashboardText, /machine release blockers/);
  assert.match(dashboardText, /draft generated programs/);
  assert.match(dashboardText, /nats result fanout/);
  assert.match(dashboardText, /mdp learning fanout/);
  assert.match(dashboardText, /Runtime CPU and Memory Limit Headroom/);
  assert.match(dashboardText, /dd_k8s_pod_container_cpu_usage_cores/);
  assert.match(dashboardText, /dd_k8s_pod_container_memory_usage_bytes/);
  assert.match(dashboardText, /Fabrication Gateway Access and Guardrail Logs/);
  assert.match(dashboardText, /Fabrication Gateway Guardrail Rejections/);
  assert.match(dashboardText, /Fabrication Gateway Edge Latency/);
  assert.match(dashboardText, /Fabrication Gateway Upstream Latency/);
  assert.match(dashboardText, /Fabrication Gateway Upstream Failures/);
  assert.match(dashboardText, /Fabrication Gateway Request Size/);
  assert.match(dashboardText, /Fabrication Gateway Response Size/);
  assert.match(dashboardText, /count_over_time/);
  assert.match(dashboardText, /quantile_over_time\(0\.95/);
  assert.match(dashboardText, /max_over_time/);
  assert.match(dashboardText, /unwrap request_time/);
  assert.match(dashboardText, /unwrap upstream_response_time/);
  assert.match(dashboardText, /unwrap request_length/);
  assert.match(dashboardText, /unwrap body_bytes_sent/);
  assert.match(dashboardText, /upstream_status/);
  assert.match(dashboardText, /upstream p95/);
  assert.match(dashboardText, /upstream max/);
  assert.match(dashboardText, /request bytes p95/);
  assert.match(dashboardText, /request bytes max/);
  assert.match(dashboardText, /response bytes p95/);
  assert.match(dashboardText, /response bytes max/);
  assert.match(dashboardText, /upstream 500/);
  assert.match(dashboardText, /upstream 502/);
  assert.match(dashboardText, /upstream 503/);
  assert.match(dashboardText, /upstream 504/);
  assert.match(dashboardText, /401 auth/);
  assert.match(dashboardText, /404 internal route/);
  assert.match(dashboardText, /405 method/);
  assert.match(dashboardText, /413 payload/);
  assert.match(dashboardText, /429 rate limit/);
  assert.match(dashboardText, /dd-remote-gateway/);
  assert.match(dashboardText, /401\|404\|405\|413\|429/);
  assert.match(observabilityReadme, /validation-finding and\s+machine-failure boundary rates/);
  assert.match(
    observabilityReadme,
    /generated-program,\s+job\/artifact, learning-event, and artifact detail-request throughput/,
  );
  assert.match(observabilityReadme, /Loki-derived gateway guardrail rejection counters/);
  assert.match(observabilityReadme, /auth\/internal-route\/method\/payload\/rate-limit failures/);
  assert.match(observabilityReadme, /gateway edge-latency/);
  assert.match(observabilityReadme, /access-log `request_time`/);
  assert.match(observabilityReadme, /upstream p95\/max panels/);
  assert.match(observabilityReadme, /upstream_response_time/);
  assert.match(observabilityReadme, /upstream 500\/502\/503\/504 failure counters/);
  assert.match(observabilityReadme, /upstream_status/);
  assert.match(observabilityReadme, /request-size p95\/max panels/);
  assert.match(observabilityReadme, /request_length/);
  assert.match(observabilityReadme, /512k/);
  assert.match(observabilityReadme, /direct pod scrape coverage/);
  assert.match(observabilityReadme, /fewer ready direct pod scrapes than desired replicas/);
  assert.match(observabilityReadme, /response-size p95\/max panels/);
  assert.match(observabilityReadme, /body_bytes_sent/);
  assert.match(gateway, /log_format dd_gateway_json escape=json/);
  assert.match(gateway, /"schema":"dd\.gateway\.access\.v1"/);
  assert.match(gateway, /"uri":"\$uri"/);
  assert.match(gateway, /access_log \/dev\/stdout dd_gateway_json/);
  const fabricationSecurityHeaderLocations = [
    "location = /fabrication {",
    "location = /fabrication/internal {",
    "location ^~ /fabrication/internal/ {",
    "location /fabrication/ {",
    "location @fabrication_payload_too_large {",
    "location @fabrication_rate_limited {",
    "location @fabrication_redirect_method_not_allowed {",
    "location @fabrication_method_not_allowed {",
  ];
  const fabricationSecurityHeaders = [
    'add_header Strict-Transport-Security "max-age=15552000" always;',
    'add_header Content-Security-Policy "upgrade-insecure-requests" always;',
    'add_header X-Frame-Options "SAMEORIGIN" always;',
    'add_header X-Content-Type-Options "nosniff" always;',
    'add_header Referrer-Policy "strict-origin-when-cross-origin" always;',
  ];
  for (const locationMarker of fabricationSecurityHeaderLocations) {
    const start = gateway.indexOf(locationMarker);
    assert.notEqual(start, -1, `expected ${locationMarker} in gateway config`);
    const end = gateway.indexOf("\n      }", start);
    assert.notEqual(end, -1, `expected closing brace for ${locationMarker}`);
    const block = gateway.slice(start, end);
    for (const header of fabricationSecurityHeaders) {
      assert.ok(block.includes(header), `expected ${locationMarker} to preserve ${header}`);
    }
  }
  assert.match(
    gateway,
    /location = \/fabrication[\s\S]*error_page 405 = @fabrication_redirect_method_not_allowed/,
  );
  assert.match(
    gateway,
    /location @fabrication_redirect_method_not_allowed[\s\S]*add_header Allow "GET, HEAD" always/,
  );
  assert.match(
    gateway,
    /location @fabrication_method_not_allowed[\s\S]*add_header Allow "GET, HEAD, POST" always/,
  );
  assert.match(
    gateway,
    /location @fabrication_rate_limited[\s\S]*add_header Retry-After 60 always/,
  );
  assert.match(runtimeReadme, /GET, HEAD` for the canonical `\/fabrication` redirect/);
  assert.match(runtimeReadme, /GET, HEAD, POST` for `\/fabrication\/`/);
  assert.match(runtimeReadme, /explicitly preserve the gateway security header set/);
  assert.match(runtimeReadme, /validation findings, machine-failure boundaries/);
  assert.match(runtimeReadme, /X-Content-Type-Options/);
  assert.match(runtimeReadme, /Retry-After: 60/);
  assert.match(dashboardText, /Kubernetes Startup, Warning, and Termination Evidence/);
  assert.match(dashboardText, /dd_k8s_pod_init_container_waiting/);
  assert.match(dashboardText, /dd_k8s_pod_init_container_restarts_total/);
  assert.match(dashboardText, /dd_k8s_event_count/);
  assert.match(dashboardText, /dd_k8s_pod_container_last_terminated/);
  assert.match(dashboardText, /dd_k8s_pod_init_container_last_terminated/);
  assert.match(dashboardText, /dd_k8s_pod_container_waiting/);
  assert.match(prometheus, /alert:\s*DDFabricationServerServingContainerWaiting/);
  assert.match(prometheus, /alert:\s*DDFabricationServerRolloutUpdatedReplicasLagging/);
  assert.match(prometheus, /alert:\s*DDFabricationServerPodScrapeCoverageBelowDesired/);
  assert.match(prometheus, /alert:\s*DDFabricationServerCpuNearLimit/);
  assert.match(prometheus, /alert:\s*DDFabricationServerMemoryNearLimit/);
  assert.match(prometheus, /alert:\s*DDFabricationServerValidationFindingsIncreasing/);
  assert.match(prometheus, /dd_fabrication_server_validation_findings_total\[10m\]/);
  assert.match(
    prometheus,
    /dd_k8s_pod_container_waiting\{namespace="default",app="dd-fabrication-server",container="fabrication-server"\}/,
  );
  assert.match(
    prometheus,
    /dd_k8s_deployment_updated_replicas\{namespace="default",deployment="dd-fabrication-server",app="dd-fabrication-server"\}/,
  );
  assert.match(
    prometheus,
    /sum\(up\{job="dd-fabrication-server-pods"\} == bool 1\) < max\(dd_k8s_deployment_desired_replicas\{namespace="default",deployment="dd-fabrication-server",app="dd-fabrication-server"\}\)/,
  );
  assert.match(
    prometheus,
    /dd_k8s_pod_container_cpu_usage_cores\{namespace="default",app="dd-fabrication-server",container="fabrication-server"\}/,
  );
  assert.match(
    prometheus,
    /dd_k8s_pod_container_memory_usage_bytes\{namespace="default",app="dd-fabrication-server",container="fabrication-server"\}/,
  );
  assert.match(dashboardText, /Fabrication Gateway/);
  assert.match(dashboardText, /\/fabrication\//);
  assert.match(dashboardText, /Fabrication API Docs/);
  assert.match(dashboardText, /\/fabrication\/docs\/api/);
  assert.match(dashboardText, /Fabrication Jobs/);
  assert.match(dashboardText, /\/fabrication\/jobs/);
  assert.match(dashboardText, /Dashboard Shortcut/);
  assert.match(dashboardText, /\/grafana\/fabrication/);
  assert.match(dashboardText, /\/grafana\/depl\/dd-fabrication-server/);
  assert.match(webHome, /async fn grafana_fabrication_redirect/);
  assert.match(webHome, /\/telemetry\/d\/dd-fabrication-planner\/fabrication-planner\?orgId=1/);
  assert.match(webHome, /\.route\("\/grafana\/fabrication", get\(grafana_fabrication_redirect\)\)/);
  assert.match(webHome, /\.route\("\/grafana\/fabrication\/", get\(grafana_fabrication_redirect\)\)/);
});
