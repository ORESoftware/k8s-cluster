import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/gleam-mcp-server/gleam.toml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('Gleam MCP server is a standalone OTP runtime', async () => {
  const gleamToml = await readRepoFile('remote/gleam-mcp-server/gleam.toml');
  const main = await readRepoFile('remote/gleam-mcp-server/src/gleam_mcp_server.gleam');
  const httpServer = await readRepoFile(
    'remote/gleam-mcp-server/src/gleam_mcp_server/http_server.gleam',
  );
  const metrics = await readRepoFile('remote/gleam-mcp-server/src/gleam_mcp_server/metrics.gleam');
  const observability = await readRepoFile(
    'remote/gleam-mcp-server/src/gleam_mcp_server/observability.gleam',
  );
  const observabilityFfi = await readRepoFile(
    'remote/gleam-mcp-server/src/gleam_mcp_observability.erl',
  );
  const runtimeEnv = await readRepoFile('remote/gleam-mcp-server/src/gleam_mcp_runtime_env.erl');

  assert.match(gleamToml, /name = "gleam_mcp_server"/);
  assert.match(gleamToml, /mist = ">= 6\.0\.0 and < 7\.0\.0"/);
  assert.match(main, /supervisor\.new\(supervisor\.OneForOne\)/);
  assert.match(main, /metrics\.start\(named_as: metrics_name\)/);
  assert.match(main, /http_server\.supervised\(metrics_name\)/);
  assert.match(httpServer, /@external\(erlang, "gleam_mcp_runtime_env", "getenv"\)/);
  assert.match(httpServer, /const default_port = 8090/);
  assert.match(httpServer, /pub fn bind_host\(\)/);
  assert.match(httpServer, /pub fn bind_port\(\)/);
  assert.match(httpServer, /const protocol_version = "2025-11-25"/);
  assert.match(main, /http_server\.bind_host\(\)/);
  assert.match(main, /http_server\.bind_port\(\)/);
  assert.match(runtimeEnv, /-module\(gleam_mcp_runtime_env\)/);
  assert.match(runtimeEnv, /os:getenv\(Name\)/);
  assert.match(httpServer, /Get, \["healthz"\] -> healthz\(\)/);
  assert.match(httpServer, /Get, \["metrics"\] -> metrics_response\(metrics_name\)/);
  assert.match(httpServer, /Get, \["observability"\] -> observability_response\(\)/);
  assert.match(httpServer, /"initialize"/);
  assert.match(httpServer, /"tools\/list"/);
  assert.match(httpServer, /"tools\/call"/);
  assert.match(httpServer, /"telemetry_summary"/);
  assert.match(httpServer, /"observability_health"/);
  assert.match(httpServer, /"prometheus_up"/);
  assert.match(httpServer, /"loki_labels"/);
  assert.match(httpServer, /"grafana_inventory"/);
  assert.match(httpServer, /"nats_metrics"/);
  assert.match(httpServer, /"trace_backends"/);
  assert.match(httpServer, /dd_gleam_mcp_rpc_requests_total/);
  assert.match(metrics, /RecordRpcRequest\(String\)/);
  assert.match(observability, /@external\(erlang, "gleam_mcp_observability", "health_json"\)/);
  assert.match(observability, /telemetry_summary_json/);
  assert.match(observability, /grafana_inventory_json/);
  assert.match(observability, /nats_metrics_json/);
  assert.match(observabilityFfi, /httpc:request/);
  assert.match(observabilityFfi, /application:ensure_all_started\(ssl\)/);
  assert.match(observabilityFfi, /parallel_checks/);
  assert.match(observabilityFfi, /timeout_check/);
  assert.match(observabilityFfi, /MCP_PROMETHEUS_URL/);
  assert.match(observabilityFfi, /MCP_LOKI_URL/);
  assert.match(observabilityFfi, /\/api\/datasources/);
  assert.match(observabilityFfi, /\/api\/search\?type=dash-db/);
  assert.match(observabilityFfi, /MCP_OTEL_COLLECTOR_URL/);
  assert.match(observabilityFfi, /MCP_NATS_MONITOR_URL/);
  assert.match(observabilityFfi, /MCP_NATS_METRICS_URL/);
  assert.match(observabilityFfi, /MCP_OBSERVABILITY_TIMEOUT_MS/);
});

test('Gleam MCP server has EC2 and minikube Kubernetes applications', async () => {
  const ec2Deployment = await readRepoFile(
    'remote/gleam-mcp-server/k8s/ec2/dd-gleam-mcp-server.deployment.yaml',
  );
  const ec2Service = await readRepoFile(
    'remote/gleam-mcp-server/k8s/ec2/dd-gleam-mcp-server.service.yaml',
  );
  const ec2NetworkPolicy = await readRepoFile(
    'remote/gleam-mcp-server/k8s/ec2/dd-gleam-mcp-server.networkpolicy.yaml',
  );
  const ec2Kustomization = await readRepoFile(
    'remote/gleam-mcp-server/k8s/ec2/kustomization.yaml',
  );
  const minikubeDeployment = await readRepoFile(
    'remote/gleam-mcp-server/k8s/minikube/dd-gleam-mcp-server.deployment.yaml',
  );
  const minikubeNetworkPolicy = await readRepoFile(
    'remote/gleam-mcp-server/k8s/minikube/dd-gleam-mcp-server.networkpolicy.yaml',
  );
  const minikubeKustomization = await readRepoFile(
    'remote/gleam-mcp-server/k8s/minikube/kustomization.yaml',
  );
  const ec2App = await readRepoFile('remote/argocd/apps/dd-gleam-mcp-server.application.yaml');
  const minikubeApp = await readRepoFile(
    'remote/argocd/apps/dd-gleam-mcp-server-minikube.application.yaml',
  );

  assert.match(ec2Deployment, /name:\s*dd-gleam-mcp-server/);
  assert.match(ec2Deployment, /ghcr\.io\/gleam-lang\/gleam:v1\.16\.0-erlang-alpine/);
  assert.match(
    ec2Deployment,
    /cd \/opt\/dd-next-1\/remote\/gleam-mcp-server[\s\S]*gleam clean \|\| true[\s\S]*gleam deps download[\s\S]*exec gleam run/,
  );
  assert.match(ec2Deployment, /gleam deps download/);
  assert.match(ec2Deployment, /exec gleam run/);
  assert.match(ec2Deployment, /containerPort:\s*8090/);
  assert.match(ec2Deployment, /capabilities:[\s\S]*drop:[\s\S]*-\s*ALL/);
  assert.match(ec2Deployment, /startupProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*8090/);
  assert.match(ec2Deployment, /startupProbe:[\s\S]*failureThreshold:\s*60/);
  assert.match(ec2Deployment, /readinessProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*8090/);
  assert.match(ec2Deployment, /livenessProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*8090/);
  assert.match(ec2Deployment, /requests:[\s\S]*cpu:\s*"1"[\s\S]*memory:\s*1Gi/);
  assert.match(ec2Deployment, /limits:[\s\S]*cpu:\s*"4"[\s\S]*memory:\s*8Gi/);
  assert.match(ec2Deployment, /mountPath:\s*\/opt\/dd-next-1/);
  assert.match(ec2Deployment, /dd\.dev\/telemetry-revision/);
  assert.match(ec2Deployment, /name:\s*HOST[\s\S]*value:\s*0\.0\.0\.0/);
  assert.match(ec2Deployment, /name:\s*PORT[\s\S]*value:\s*'8090'/);
  assert.match(ec2Deployment, /MCP_PROMETHEUS_URL[\s\S]*dd-prometheus\.observability\.svc\.cluster\.local:9090/);
  assert.match(ec2Deployment, /MCP_LOKI_URL[\s\S]*dd-loki\.observability\.svc\.cluster\.local:3100/);
  assert.match(ec2Deployment, /MCP_GRAFANA_URL[\s\S]*dd-grafana\.observability\.svc\.cluster\.local:3000/);
  assert.match(ec2Deployment, /MCP_TEMPO_URL[\s\S]*dd-tempo\.observability\.svc\.cluster\.local:3200/);
  assert.match(ec2Deployment, /MCP_JAEGER_URL[\s\S]*dd-jaeger\.observability\.svc\.cluster\.local:16686/);
  assert.match(ec2Deployment, /MCP_OTEL_COLLECTOR_URL[\s\S]*dd-otel-collector\.observability\.svc\.cluster\.local:8889/);
  assert.match(ec2Deployment, /MCP_NATS_MONITOR_URL[\s\S]*dd-nats\.messaging\.svc\.cluster\.local:8222/);
  assert.match(ec2Deployment, /MCP_NATS_METRICS_URL[\s\S]*dd-nats\.messaging\.svc\.cluster\.local:7777/);
  assert.match(ec2Deployment, /MCP_OBSERVABILITY_TIMEOUT_MS[\s\S]*value:\s*'1200'/);
  assert.match(ec2Deployment, /MCP_OBSERVABILITY_BODY_LIMIT_BYTES[\s\S]*value:\s*'32768'/);
  assert.match(ec2Deployment, /path:\s*\/home\/ec2-user\/codes\/dd\/dd-next-1/);
  assert.match(ec2Service, /port:\s*8090/);
  assert.match(ec2Service, /targetPort:\s*8090/);
  assert.match(ec2Kustomization, /dd-gleam-mcp-server\.networkpolicy\.yaml/);
  assert.match(ec2NetworkPolicy, /kind:\s*NetworkPolicy/);
  assert.match(ec2NetworkPolicy, /app:\s*dd-gleam-mcp-server/);
  assert.match(ec2NetworkPolicy, /app:\s*dd-remote-gateway/);
  assert.match(ec2NetworkPolicy, /kubernetes\.io\/metadata\.name:\s*observability/);
  assert.match(ec2NetworkPolicy, /kubernetes\.io\/metadata\.name:\s*messaging/);
  assert.match(ec2NetworkPolicy, /app:\s*dd-nats/);
  assert.match(ec2NetworkPolicy, /port:\s*3000/);
  assert.match(ec2NetworkPolicy, /port:\s*3100/);
  assert.match(ec2NetworkPolicy, /port:\s*3200/);
  assert.match(ec2NetworkPolicy, /port:\s*7777/);
  assert.match(ec2NetworkPolicy, /port:\s*8222/);
  assert.match(ec2NetworkPolicy, /port:\s*8889/);
  assert.match(ec2NetworkPolicy, /port:\s*9090/);
  assert.match(ec2NetworkPolicy, /port:\s*16686/);
  assert.match(ec2NetworkPolicy, /port:\s*5432/);
  assert.match(minikubeDeployment, /image:\s*dd-gleam-mcp-server:dev/);
  assert.match(minikubeDeployment, /name:\s*HOST[\s\S]*value:\s*0\.0\.0\.0/);
  assert.match(minikubeDeployment, /name:\s*PORT[\s\S]*value:\s*'8090'/);
  assert.match(minikubeDeployment, /MCP_PROMETHEUS_URL[\s\S]*dd-prometheus\.observability\.svc\.cluster\.local:9090/);
  assert.match(minikubeDeployment, /MCP_LOKI_URL[\s\S]*dd-loki\.observability\.svc\.cluster\.local:3100/);
  assert.match(minikubeDeployment, /MCP_OTEL_COLLECTOR_URL[\s\S]*dd-otel-collector\.observability\.svc\.cluster\.local:8889/);
  assert.match(minikubeDeployment, /MCP_NATS_MONITOR_URL[\s\S]*dd-nats\.messaging\.svc\.cluster\.local:8222/);
  assert.match(minikubeDeployment, /MCP_NATS_METRICS_URL[\s\S]*dd-nats\.messaging\.svc\.cluster\.local:7777/);
  assert.match(minikubeDeployment, /startupProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*8090/);
  assert.match(minikubeDeployment, /startupProbe:[\s\S]*failureThreshold:\s*60/);
  assert.match(minikubeDeployment, /readinessProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*8090/);
  assert.match(minikubeDeployment, /livenessProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*8090/);
  assert.match(minikubeKustomization, /dd-gleam-mcp-server\.networkpolicy\.yaml/);
  assert.match(minikubeNetworkPolicy, /namespace:\s*dd-dev-local/);
  assert.match(minikubeNetworkPolicy, /kubernetes\.io\/metadata\.name:\s*observability/);
  assert.match(minikubeNetworkPolicy, /kubernetes\.io\/metadata\.name:\s*messaging/);
  assert.match(minikubeNetworkPolicy, /port:\s*7777/);
  assert.match(minikubeNetworkPolicy, /port:\s*8222/);
  assert.match(ec2App, /path:\s*remote\/gleam-mcp-server\/k8s\/ec2/);
  assert.match(minikubeApp, /path:\s*remote\/gleam-mcp-server\/k8s\/minikube/);
});

test('Gleam MCP server is exposed through gateway and observability', async () => {
  const gateway = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml',
  );
  const collector = await readRepoFile(
    'remote/argocd/observability/otel-collector.configmap.yaml',
  );
  const dashboard = await readRepoFile(
    'remote/argocd/observability/grafana.dashboards.configmap.yaml',
  );

  assert.match(gateway, /location = \/mcp/);
  assert.match(gateway, /location\s+\/mcp\//);
  assert.match(gateway, /dd-gleam-mcp-server\.default\.svc\.cluster\.local:8090/);
  assert.match(gateway, /X-Forwarded-Prefix \/mcp/);
  assert.match(collector, /job_name: dd-gleam-mcp-server/);
  assert.match(collector, /job_name: dd-gleam-mcp-server[\s\S]*metrics_path: \/metrics/);
  assert.match(collector, /dd-gleam-mcp-server\.default\.svc\.cluster\.local:8090/);
  assert.match(dashboard, /Gleam MCP Runtime/);
  assert.match(dashboard, /dd_gleam_mcp_rpc_requests_total/);
});

test('Gleam MCP server exposes read-only observability tools', async () => {
  const httpServer = await readRepoFile(
    'remote/gleam-mcp-server/src/gleam_mcp_server/http_server.gleam',
  );
  const readme = await readRepoFile('remote/gleam-mcp-server/readme.md');

  assert.match(httpServer, /observability\.health_json\(\)/);
  assert.match(httpServer, /observability\.telemetry_summary_json\(\)/);
  assert.match(httpServer, /observability\.prometheus_up_json\(\)/);
  assert.match(httpServer, /observability\.loki_labels_json\(\)/);
  assert.match(httpServer, /observability\.grafana_inventory_json\(\)/);
  assert.match(httpServer, /observability\.nats_metrics_json\(\)/);
  assert.match(httpServer, /observability\.trace_backends_json\(\)/);
  assert.match(httpServer, /openWorldHint\\":false/);
  assert.match(readme, /telemetry_summary/);
  assert.match(readme, /observability_health/);
  assert.match(readme, /prometheus_up/);
  assert.match(readme, /loki_labels/);
  assert.match(readme, /grafana_inventory/);
  assert.match(readme, /nats_metrics/);
  assert.match(readme, /trace_backends/);
  assert.match(readme, /Grafana datasource and dashboard inventory/);
  assert.match(readme, /MCP_NATS_MONITOR_URL/);
  assert.match(readme, /MCP_NATS_METRICS_URL/);
  assert.match(readme, /fan out checks\s+in parallel/);
  assert.match(readme, /NetworkPolicy/);
  assert.match(readme, /does not need Kubernetes\s+API permissions/);
});
