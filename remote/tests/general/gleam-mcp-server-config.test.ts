import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

const repoRoot = resolve(process.cwd(), '..', '..');

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('Gleam MCP server is a standalone OTP runtime', async () => {
  const gleamToml = await readRepoFile('remote/gleam-mcp-server/gleam.toml');
  const main = await readRepoFile('remote/gleam-mcp-server/src/gleam_mcp_server.gleam');
  const httpServer = await readRepoFile(
    'remote/gleam-mcp-server/src/gleam_mcp_server/http_server.gleam',
  );
  const observability = await readRepoFile('remote/gleam-mcp-server/src/mcp_observability.erl');
  const metrics = await readRepoFile('remote/gleam-mcp-server/src/gleam_mcp_server/metrics.gleam');

  assert.match(gleamToml, /name = "gleam_mcp_server"/);
  assert.match(gleamToml, /mist = ">= 6\.0\.0 and < 7\.0\.0"/);
  assert.match(main, /supervisor\.new\(supervisor\.OneForOne\)/);
  assert.match(main, /metrics\.start\(named_as: metrics_name\)/);
  assert.match(main, /http_server\.supervised\(metrics_name\)/);
  assert.match(httpServer, /const port = 8090/);
  assert.match(httpServer, /const protocol_version = "2025-11-25"/);
  assert.match(httpServer, /Get, \["healthz"\] -> healthz\(\)/);
  assert.match(httpServer, /Get, \["metrics"\] -> metrics_response\(metrics_name\)/);
  assert.match(httpServer, /Get, \["observability"\] -> observability_response\(\)/);
  assert.match(httpServer, /observability_snapshot/);
  assert.match(httpServer, /@external\(erlang, "mcp_observability", "snapshot"\)/);
  assert.match(httpServer, /"initialize"/);
  assert.match(httpServer, /"tools\/list"/);
  assert.match(httpServer, /"tools\/call"/);
  assert.match(httpServer, /dd_gleam_mcp_rpc_requests_total/);
  assert.match(observability, /MCP_PROMETHEUS_URL/);
  assert.match(observability, /MCP_LOKI_URL/);
  assert.match(observability, /MCP_GRAFANA_URL/);
  assert.match(observability, /MCP_OTEL_COLLECTOR_URL/);
  assert.match(observability, /MCP_TEMPO_URL/);
  assert.match(observability, /MCP_JAEGER_URL/);
  assert.match(observability, /MCP_NATS_METRICS_URL/);
  assert.match(observability, /MCP_OBS_HTTP_TIMEOUT_MS/);
  assert.match(observability, /MCP_OBS_SNAPSHOT_TIMEOUT_MS/);
  assert.match(observability, /MCP_OBS_SNIPPET_BYTES/);
  assert.match(observability, /start_target_jobs/);
  assert.match(observability, /collect_target_jobs/);
  assert.match(observability, /read_health_and_data/);
  assert.match(observability, /collect_http_result/);
  assert.match(observability, /safe_http_get/);
  assert.match(observability, /snippetBytes/);
  assert.match(observability, /user-agent/);
  assert.match(observability, /httpc:request/);
  assert.match(observability, /DEFAULT_MAX_SNIPPET_BYTES/);
  assert.match(metrics, /RecordRpcRequest\(String\)/);
});

test('Gleam MCP server has EC2 and minikube Kubernetes applications', async () => {
  const ec2Deployment = await readRepoFile(
    'remote/gleam-mcp-server/k8s/ec2/dd-gleam-mcp-server.deployment.yaml',
  );
  const ec2Service = await readRepoFile(
    'remote/gleam-mcp-server/k8s/ec2/dd-gleam-mcp-server.service.yaml',
  );
  const minikubeDeployment = await readRepoFile(
    'remote/gleam-mcp-server/k8s/minikube/dd-gleam-mcp-server.deployment.yaml',
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
  assert.match(ec2Deployment, /MCP_PROMETHEUS_URL[\s\S]*dd-prometheus\.observability\.svc\.cluster\.local:9090/);
  assert.match(ec2Deployment, /MCP_LOKI_URL[\s\S]*dd-loki\.observability\.svc\.cluster\.local:3100/);
  assert.match(ec2Deployment, /MCP_GRAFANA_URL[\s\S]*dd-grafana\.observability\.svc\.cluster\.local:3000/);
  assert.match(ec2Deployment, /MCP_OTEL_COLLECTOR_URL[\s\S]*dd-otel-collector\.observability\.svc\.cluster\.local:8889/);
  assert.match(ec2Deployment, /MCP_TEMPO_URL[\s\S]*dd-tempo\.observability\.svc\.cluster\.local:3200/);
  assert.match(ec2Deployment, /MCP_JAEGER_URL[\s\S]*dd-jaeger\.observability\.svc\.cluster\.local:16686/);
  assert.match(ec2Deployment, /MCP_NATS_METRICS_URL[\s\S]*dd-nats\.messaging\.svc\.cluster\.local:7777/);
  assert.match(ec2Deployment, /MCP_OBS_HTTP_TIMEOUT_MS[\s\S]*value:\s*"650"/);
  assert.match(ec2Deployment, /MCP_OBS_SNAPSHOT_TIMEOUT_MS[\s\S]*value:\s*"1800"/);
  assert.match(ec2Deployment, /MCP_OBS_SNIPPET_BYTES[\s\S]*value:\s*"768"/);
  assert.match(ec2Deployment, /readinessProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*8090/);
  assert.match(ec2Deployment, /livenessProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*8090/);
  assert.match(ec2Deployment, /requests:[\s\S]*cpu:\s*"1"[\s\S]*memory:\s*1Gi/);
  assert.match(ec2Deployment, /limits:[\s\S]*cpu:\s*"4"[\s\S]*memory:\s*8Gi/);
  assert.match(ec2Deployment, /mountPath:\s*\/opt\/dd-next-1/);
  assert.match(ec2Deployment, /dd\.dev\/telemetry-revision/);
  assert.match(ec2Deployment, /path:\s*\/home\/ec2-user\/codes\/dd\/dd-next-1/);
  assert.match(ec2Service, /port:\s*8090/);
  assert.match(ec2Service, /targetPort:\s*8090/);
  assert.match(minikubeDeployment, /image:\s*dd-gleam-mcp-server:dev/);
  assert.match(minikubeDeployment, /MCP_PROMETHEUS_URL[\s\S]*dd-prometheus\.observability\.svc\.cluster\.local:9090/);
  assert.match(minikubeDeployment, /MCP_LOKI_URL[\s\S]*dd-loki\.observability\.svc\.cluster\.local:3100/);
  assert.match(minikubeDeployment, /MCP_GRAFANA_URL[\s\S]*dd-grafana\.observability\.svc\.cluster\.local:3000/);
  assert.match(minikubeDeployment, /MCP_OTEL_COLLECTOR_URL[\s\S]*dd-otel-collector\.observability\.svc\.cluster\.local:8889/);
  assert.match(minikubeDeployment, /MCP_TEMPO_URL[\s\S]*dd-tempo\.observability\.svc\.cluster\.local:3200/);
  assert.match(minikubeDeployment, /MCP_JAEGER_URL[\s\S]*dd-jaeger\.observability\.svc\.cluster\.local:16686/);
  assert.match(minikubeDeployment, /MCP_NATS_METRICS_URL[\s\S]*dd-nats\.messaging\.svc\.cluster\.local:7777/);
  assert.match(minikubeDeployment, /MCP_OBS_HTTP_TIMEOUT_MS[\s\S]*value:\s*"650"/);
  assert.match(minikubeDeployment, /MCP_OBS_SNAPSHOT_TIMEOUT_MS[\s\S]*value:\s*"1800"/);
  assert.match(minikubeDeployment, /MCP_OBS_SNIPPET_BYTES[\s\S]*value:\s*"768"/);
  assert.match(minikubeDeployment, /readinessProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*8090/);
  assert.match(minikubeDeployment, /livenessProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*8090/);
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
