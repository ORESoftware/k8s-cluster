import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/deployments/gleam-mcp-server/gleam.toml'))) {
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
  const gleamToml = await readRepoFile('remote/deployments/gleam-mcp-server/gleam.toml');
  const main = await readRepoFile('remote/deployments/gleam-mcp-server/src/gleam_mcp_server.gleam');
  const httpServer = await readRepoFile(
    'remote/deployments/gleam-mcp-server/src/gleam_mcp_server/http_server.gleam',
  );
  const metrics = await readRepoFile('remote/deployments/gleam-mcp-server/src/gleam_mcp_server/metrics.gleam');
  const observability = await readRepoFile(
    'remote/deployments/gleam-mcp-server/src/gleam_mcp_server/observability.gleam',
  );
  const k8s = await readRepoFile('remote/deployments/gleam-mcp-server/src/gleam_mcp_server/k8s.gleam');
  const observabilityFfi = await readRepoFile(
    'remote/deployments/gleam-mcp-server/src/gleam_mcp_observability.erl',
  );
  const k8sFfi = await readRepoFile('remote/deployments/gleam-mcp-server/src/gleam_mcp_k8s.erl');
  const runtimeEnv = await readRepoFile('remote/deployments/gleam-mcp-server/src/gleam_mcp_runtime_env.erl');
  const jsonFfi = await readRepoFile('remote/deployments/gleam-mcp-server/src/gleam_mcp_json.erl');

  assert.match(gleamToml, /name = "gleam_mcp_server"/);
  assert.match(gleamToml, /mist = ">= 6\.0\.0 and < 7\.0\.0"/);
  assert.match(main, /supervisor\.new\(supervisor\.OneForOne\)/);
  assert.match(main, /metrics\.start\(named_as: metrics_name\)/);
  assert.match(main, /http_server\.supervised\(metrics_name\)/);
  assert.match(httpServer, /@external\(erlang, "gleam_mcp_runtime_env", "getenv"\)/);
  assert.match(httpServer, /@external\(erlang, "gleam_mcp_json", "request_id"\)/);
  assert.match(httpServer, /const default_port = 8090/);
  assert.match(httpServer, /pub fn bind_host\(\)/);
  assert.match(httpServer, /pub fn bind_port\(\)/);
  assert.match(httpServer, /const protocol_version = "2025-11-25"/);
  assert.match(main, /http_server\.bind_host\(\)/);
  assert.match(main, /http_server\.bind_port\(\)/);
  assert.match(runtimeEnv, /-module\(gleam_mcp_runtime_env\)/);
  assert.match(runtimeEnv, /os:getenv\(Name\)/);
  assert.match(jsonFfi, /-module\(gleam_mcp_json\)/);
  assert.match(jsonFfi, /request_id\/1/);
  assert.match(jsonFfi, /re:run\(Body, Pattern/);
  assert.match(httpServer, /Get, \["healthz"\] -> healthz\(\)/);
  assert.match(httpServer, /Get, \["metrics"\] -> metrics_response\(metrics_name\)/);
  assert.match(httpServer, /Get, \["observability"\] -> observability_response\(\)/);
  assert.match(httpServer, /"initialize"/);
  assert.match(httpServer, /"tools\/list"/);
  assert.match(httpServer, /"tools\/call"/);
  assert.match(httpServer, /initialize_result\(request_id\)/);
  assert.match(httpServer, /tools_list_result\(request_id\)/);
  assert.match(httpServer, /tools_call_result\(tool_from_body\(body\), request_id\)/);
  assert.match(httpServer, /import gleam_mcp_server\/k8s/);
  assert.match(httpServer, /"kubernetes_inventory"/);
  assert.match(httpServer, /"kubernetes_deployments"/);
  assert.match(httpServer, /"human_access_policy"/);
  assert.match(httpServer, /k8s\.inventory_json\(\)/);
  assert.match(httpServer, /k8s\.deployments_json\(\)/);
  assert.match(httpServer, /k8s\.inventory_json\(\)/);
  assert.match(httpServer, /k8s\.human_access_policy_json\(\)/);
  assert.match(httpServer, /k8s\.human_access_policy_json\(\)/);
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
  assert.match(k8s, /@external\(erlang, "gleam_mcp_k8s", "deployments_json"\)/);
  assert.match(k8s, /inventory_json/);
  assert.match(k8s, /human_access_policy_json/);
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
  assert.match(k8sFfi, /-module\(gleam_mcp_k8s\)/);
  assert.match(k8sFfi, /inventory_json\/0/);
  assert.match(k8sFfi, /human_access_policy_json\/0/);
  assert.match(k8sFfi, /\/apis\/apps\/v1\/deployments\?limit=500/);
  assert.match(k8sFfi, /\/api\/v1\/pods\?limit=500/);
  assert.match(k8sFfi, /\/api\/v1\/nodes\?limit=500/);
  assert.match(k8sFfi, /\/apis\/networking\.k8s\.io\/v1\/ingresses\?limit=500/);
  assert.match(k8sFfi, /DD_AUTH_TOTP_SECRET_BASE32|operator passphrase plus optional TOTP/);
  assert.match(k8sFfi, /MCP_KUBERNETES_API_URL/);
  assert.match(k8sFfi, /MCP_KUBERNETES_TOKEN_PATH/);
  assert.match(k8sFfi, /MCP_KUBERNETES_CA_PATH/);
  assert.match(k8sFfi, /authorization/);
  assert.match(k8sFfi, /application\/json;as=PartialObjectMetadataList;g=meta\.k8s\.io;v=v1/);
  assert.doesNotMatch(k8sFfi, /PartialObjectMetadataList;g=meta\.k8s\.io;v=v1, application\/json/);
  assert.match(k8sFfi, /httpc:request/);
});

test('Gleam MCP server uses EC2 inventory RBAC and keeps minikube narrow', async () => {
  const ec2Deployment = await readRepoFile(
    'remote/deployments/gleam-mcp-server/k8s/ec2/dd-gleam-mcp-server.deployment.yaml',
  );
  const ec2Service = await readRepoFile(
    'remote/deployments/gleam-mcp-server/k8s/ec2/dd-gleam-mcp-server.service.yaml',
  );
  const ec2NetworkPolicy = await readRepoFile(
    'remote/deployments/gleam-mcp-server/k8s/ec2/dd-gleam-mcp-server.networkpolicy.yaml',
  );
  const ec2Rbac = await readRepoFile(
    'remote/deployments/gleam-mcp-server/k8s/ec2/dd-gleam-mcp-server-rbac.yaml',
  );
  const ec2Kustomization = await readRepoFile(
    'remote/deployments/gleam-mcp-server/k8s/ec2/kustomization.yaml',
  );
  const minikubeDeployment = await readRepoFile(
    'remote/deployments/gleam-mcp-server/k8s/minikube/dd-gleam-mcp-server.deployment.yaml',
  );
  const minikubeNetworkPolicy = await readRepoFile(
    'remote/deployments/gleam-mcp-server/k8s/minikube/dd-gleam-mcp-server.networkpolicy.yaml',
  );
  const minikubeRbac = await readRepoFile(
    'remote/deployments/gleam-mcp-server/k8s/minikube/dd-gleam-mcp-server-rbac.yaml',
  );
  const minikubeKustomization = await readRepoFile(
    'remote/deployments/gleam-mcp-server/k8s/minikube/kustomization.yaml',
  );
  const ec2App = await readRepoFile('remote/argocd/apps/dd-gleam-mcp-server.application.yaml');
  const minikubeApp = await readRepoFile(
    'remote/argocd/apps/dd-gleam-mcp-server-minikube.application.yaml',
  );
  const ec2Verifier = await readRepoFile('remote/ec2/verify-gleam-mcp-server.sh');

  assert.match(ec2Deployment, /name:\s*dd-gleam-mcp-server/);
  assert.match(ec2Deployment, /ghcr\.io\/gleam-lang\/gleam:v1\.16\.0-erlang-alpine/);
  assert.match(
    ec2Deployment,
    /SRC_ROOT=\/opt\/dd-next-1[\s\S]*WORK_ROOT=\/tmp\/dd-gleam-mcp-server\/dd-next-1[\s\S]*cp -R "\$SRC_ROOT\/remote\/deployments\/gleam-mcp-server"\/\.[\s\S]*cp -R "\$SRC_ROOT\/remote\/libs\/pg-defs\/generated\/gleam"[\s\S]*exec gleam run/,
  );
  assert.doesNotMatch(ec2Deployment, /apk add/);
  assert.doesNotMatch(ec2Deployment, /^\s*gleam deps download\s*$/m);
  assert.match(ec2Deployment, /exec gleam run/);
  assert.match(ec2Deployment, /containerPort:\s*8090/);
  assert.match(ec2Deployment, /serviceAccountName:\s*dd-gleam-mcp-server/);
  assert.match(ec2Deployment, /automountServiceAccountToken:\s*true/);
  assert.match(ec2Deployment, /capabilities:[\s\S]*drop:[\s\S]*-\s*ALL/);
  assert.match(ec2Deployment, /startupProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*8090/);
  assert.match(ec2Deployment, /startupProbe:[\s\S]*failureThreshold:\s*60/);
  assert.match(ec2Deployment, /readinessProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*8090/);
  assert.match(ec2Deployment, /livenessProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*8090/);
  assert.match(ec2Deployment, /requests:[\s\S]*cpu:\s*250m[\s\S]*memory:\s*1Gi/);
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
  assert.match(ec2Deployment, /MCP_KUBERNETES_TIMEOUT_MS[\s\S]*value:\s*'1500'/);
  assert.match(ec2Deployment, /MCP_KUBERNETES_BODY_LIMIT_BYTES[\s\S]*value:\s*'262144'/);
  assert.match(ec2Deployment, /MCP_KUBERNETES_INVENTORY_BODY_LIMIT_BYTES[\s\S]*value:\s*'32768'/);
  assert.match(ec2Deployment, /path:\s*\/home\/ec2-user\/codes\/dd\/dd-next-1/);
  assert.match(ec2Service, /port:\s*8090/);
  assert.match(ec2Service, /targetPort:\s*8090/);
  assert.match(ec2Kustomization, /dd-gleam-mcp-server\.networkpolicy\.yaml/);
  assert.match(ec2Kustomization, /dd-gleam-mcp-server-rbac\.yaml/);
  assert.match(ec2Rbac, /kind:\s*ServiceAccount[\s\S]*name:\s*dd-gleam-mcp-server/);
  assert.match(ec2Rbac, /kind:\s*ClusterRole[\s\S]*name:\s*dd-gleam-mcp-server-read-inventory/);
  assert.match(ec2Rbac, /resources:[\s\S]*-\s*namespaces[\s\S]*-\s*nodes[\s\S]*-\s*pods[\s\S]*-\s*services/);
  assert.match(ec2Rbac, /apiGroups:[\s\S]*-\s*apps/);
  assert.match(ec2Rbac, /resources:[\s\S]*-\s*daemonsets[\s\S]*-\s*deployments[\s\S]*-\s*replicasets[\s\S]*-\s*statefulsets/);
  assert.match(ec2Rbac, /apiGroups:[\s\S]*-\s*batch[\s\S]*resources:[\s\S]*-\s*cronjobs[\s\S]*-\s*jobs/);
  assert.match(ec2Rbac, /apiGroups:[\s\S]*-\s*networking\.k8s\.io[\s\S]*resources:[\s\S]*-\s*ingresses[\s\S]*-\s*networkpolicies/);
  assert.match(ec2Rbac, /apiGroups:[\s\S]*-\s*apiextensions\.k8s\.io[\s\S]*customresourcedefinitions/);
  assert.match(ec2Rbac, /verbs:[\s\S]*-\s*list/);
  assert.doesNotMatch(ec2Rbac, /-\s*get/);
  assert.doesNotMatch(ec2Rbac, /-\s*secrets|-\s*configmaps|pods\/exec|pods\/log|-\s*create|-\s*patch|-\s*update|-\s*delete/);
  assert.match(ec2NetworkPolicy, /kind:\s*NetworkPolicy/);
  assert.match(ec2NetworkPolicy, /app:\s*dd-gleam-mcp-server/);
  assert.match(ec2NetworkPolicy, /app:\s*dd-remote-gateway/);
  assert.match(ec2NetworkPolicy, /app:\s*dd-dev-server-api/);
  assert.match(
    ec2NetworkPolicy,
    /app\.kubernetes\.io\/part-of:\s*dd-remote-dev[\s\S]*app\.kubernetes\.io\/component:\s*thread-pod/,
  );
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
  assert.match(ec2NetworkPolicy, /port:\s*443/);
  assert.match(ec2NetworkPolicy, /port:\s*5432/);
  assert.match(minikubeDeployment, /image:\s*dd-gleam-mcp-server:dev/);
  assert.match(minikubeDeployment, /serviceAccountName:\s*dd-gleam-mcp-server/);
  assert.match(minikubeDeployment, /automountServiceAccountToken:\s*true/);
  assert.match(minikubeDeployment, /name:\s*HOST[\s\S]*value:\s*0\.0\.0\.0/);
  assert.match(minikubeDeployment, /name:\s*PORT[\s\S]*value:\s*'8090'/);
  assert.match(minikubeDeployment, /MCP_PROMETHEUS_URL[\s\S]*dd-prometheus\.observability\.svc\.cluster\.local:9090/);
  assert.match(minikubeDeployment, /MCP_LOKI_URL[\s\S]*dd-loki\.observability\.svc\.cluster\.local:3100/);
  assert.match(minikubeDeployment, /MCP_OTEL_COLLECTOR_URL[\s\S]*dd-otel-collector\.observability\.svc\.cluster\.local:8889/);
  assert.match(minikubeDeployment, /MCP_NATS_MONITOR_URL[\s\S]*dd-nats\.messaging\.svc\.cluster\.local:8222/);
  assert.match(minikubeDeployment, /MCP_NATS_METRICS_URL[\s\S]*dd-nats\.messaging\.svc\.cluster\.local:7777/);
  assert.match(minikubeDeployment, /MCP_KUBERNETES_TIMEOUT_MS[\s\S]*value:\s*'1500'/);
  assert.match(minikubeDeployment, /MCP_KUBERNETES_BODY_LIMIT_BYTES[\s\S]*value:\s*'262144'/);
  assert.doesNotMatch(minikubeDeployment, /MCP_KUBERNETES_INVENTORY_BODY_LIMIT_BYTES/);
  assert.match(minikubeDeployment, /startupProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*8090/);
  assert.match(minikubeDeployment, /startupProbe:[\s\S]*failureThreshold:\s*60/);
  assert.match(minikubeDeployment, /readinessProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*8090/);
  assert.match(minikubeDeployment, /livenessProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*8090/);
  assert.match(minikubeKustomization, /dd-gleam-mcp-server\.networkpolicy\.yaml/);
  assert.match(minikubeKustomization, /dd-gleam-mcp-server-rbac\.yaml/);
  assert.match(minikubeRbac, /namespace:\s*dd-dev-local/);
  assert.match(minikubeRbac, /dd-gleam-mcp-server-read-deployments-local/);
  assert.match(minikubeRbac, /resources:[\s\S]*-\s*deployments/);
  assert.match(minikubeRbac, /verbs:[\s\S]*-\s*get[\s\S]*-\s*list/);
  assert.doesNotMatch(
    minikubeRbac,
    /-\s*secrets|-\s*configmaps|-\s*pods|-\s*services|-\s*nodes|customresourcedefinitions|pods\/exec|pods\/log|-\s*create|-\s*patch|-\s*update|-\s*delete/,
  );
  assert.match(minikubeNetworkPolicy, /namespace:\s*dd-dev-local/);
  assert.match(minikubeNetworkPolicy, /kubernetes\.io\/metadata\.name:\s*observability/);
  assert.match(minikubeNetworkPolicy, /kubernetes\.io\/metadata\.name:\s*messaging/);
  assert.match(minikubeNetworkPolicy, /app:\s*dd-dev-server-api/);
  assert.match(
    minikubeNetworkPolicy,
    /app\.kubernetes\.io\/part-of:\s*dd-remote-dev[\s\S]*app\.kubernetes\.io\/component:\s*thread-pod/,
  );
  assert.match(minikubeNetworkPolicy, /port:\s*7777/);
  assert.match(minikubeNetworkPolicy, /port:\s*8222/);
  assert.match(minikubeNetworkPolicy, /port:\s*443/);
  assert.match(ec2App, /path:\s*remote\/deployments\/gleam-mcp-server\/k8s\/ec2/);
  assert.match(minikubeApp, /path:\s*remote\/deployments\/gleam-mcp-server\/k8s\/minikube/);
  assert.match(ec2Verifier, /^#!\/usr\/bin\/env bash/m);
  assert.match(ec2Verifier, /remote\/argocd\/apps\/dd-gleam-mcp-server\.application\.yaml/);
  assert.match(ec2Verifier, /MCP_EXPECTED_APP_PATH:-remote\/deployments\/gleam-mcp-server\/k8s\/ec2/);
  assert.match(ec2Verifier, /\*minikube\*\|\*kind\*\|\*docker-desktop\*\|\*colima\*/);
  assert.match(ec2Verifier, /ALLOW_NON_EC2_CONTEXT=true/);
  assert.match(ec2Verifier, /kubectl -n "\$\{argocd_namespace\}" get application "\$\{app_name\}"/);
  assert.match(ec2Verifier, /kubectl apply -k "\$\{repo_root\}\/\$\{expected_app_path\}"/);
  assert.match(ec2Verifier, /kubectl auth can-i/);
  assert.match(ec2Verifier, /require_can_i list deployments\.apps --all-namespaces/);
  assert.match(ec2Verifier, /require_can_i list customresourcedefinitions\.apiextensions\.k8s\.io/);
  assert.match(ec2Verifier, /require_cannot_i list secrets --all-namespaces/);
  assert.match(ec2Verifier, /require_cannot_i list configmaps --all-namespaces/);
  assert.match(ec2Verifier, /require_cannot_i get pods\/log/);
  assert.match(ec2Verifier, /require_cannot_i create pods\/exec/);
  assert.match(ec2Verifier, /mcp_rbac_diagnostics/);
  assert.match(ec2Verifier, /kubectl auth can-i --list --as="\$\{service_account\}"/);
  assert.match(ec2Verifier, /kubectl get rolebindings -A -o json/);
  assert.match(ec2Verifier, /kubectl get clusterrolebindings -o json/);
  assert.match(ec2Verifier, /human_access_policy/);
  assert.match(ec2Verifier, /kubernetes_inventory/);
  assert.match(ec2Verifier, /--data '\{"jsonrpc":"2\.0","id":42,"method":"tools\/list"\}'/);
  assert.match(ec2Verifier, /grep -q '"id":42'/);
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
    'remote/deployments/gleam-mcp-server/src/gleam_mcp_server/http_server.gleam',
  );
  const readme = await readRepoFile('remote/deployments/gleam-mcp-server/readme.md');

  assert.match(httpServer, /k8s\.deployments_json\(\)/);
  assert.match(httpServer, /observability\.health_json\(\)/);
  assert.match(httpServer, /observability\.telemetry_summary_json\(\)/);
  assert.match(httpServer, /observability\.prometheus_up_json\(\)/);
  assert.match(httpServer, /observability\.loki_labels_json\(\)/);
  assert.match(httpServer, /observability\.grafana_inventory_json\(\)/);
  assert.match(httpServer, /observability\.nats_metrics_json\(\)/);
  assert.match(httpServer, /observability\.trace_backends_json\(\)/);
  assert.match(httpServer, /openWorldHint\\":false/);
  assert.match(readme, /telemetry_summary/);
  assert.match(readme, /kubernetes_inventory/);
  assert.match(readme, /kubernetes_deployments/);
  assert.match(readme, /human_access_policy/);
  assert.match(readme, /read-only service account/);
  assert.match(readme, /DD_AUTH_TOTP_SECRET_BASE32/);
  assert.match(readme, /MCP_KUBERNETES_API_URL/);
  assert.match(readme, /MCP_KUBERNETES_INVENTORY_BODY_LIMIT_BYTES/);
  assert.match(readme, /EC2 is the production\/canonical target for MCP/);
  assert.match(readme, /remote\/ec2\/verify-gleam-mcp-server\.sh/);
  assert.match(readme, /minikube overlay is only a local mirror/);
  assert.match(readme, /Kubernetes API egress on TCP 443/);
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
  assert.doesNotMatch(readme, /does not need Kubernetes\s+API permissions/);
});
