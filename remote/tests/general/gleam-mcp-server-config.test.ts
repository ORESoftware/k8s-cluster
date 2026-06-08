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
  assert.match(gleamToml, /dd_cli_config_client/);
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
  assert.match(runtimeEnv, /dd_cli_config_client_ffi:getenv\(Name, <<>>\)/);
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

test('Gleam MCP server uses EC2 inventory RBAC', async () => {
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
  const ec2Pdb = await readRepoFile(
    'remote/deployments/gleam-mcp-server/k8s/ec2/dd-gleam-mcp-server.pdb.yaml',
  );
  const ec2App = await readRepoFile('remote/argocd/apps/dd-gleam-mcp-server.application.yaml');
  const ec2Verifier = await readRepoFile('remote/ec2/verify-gleam-mcp-server.sh');

  assert.match(ec2Deployment, /name:\s*dd-gleam-mcp-server/);
  assert.match(ec2Deployment, /ghcr\.io\/gleam-lang\/gleam:v1\.16\.0-erlang-alpine/);
  // The EC2 overlay mounts the host checkout read-modify-unsafe at
  // /opt/dd-next-1 and drops ALL Linux capabilities, so it must copy the
  // project plus its local-path deps into a writable scratch dir backed
  // by an explicit, bounded emptyDir before invoking gleam (gleam needs
  // to write build/dev/erlang/... and the host dir is owned by ec2-user
  // with no CAP_DAC_OVERRIDE in the pod). Running `gleam run` straight
  // out of /opt/dd-next-1 silently crash-loops the pod, which surfaces
  // as a 502 at the public gateway — see commit 073e451 and its revert
  // for the failure mode.
  assert.match(
    ec2Deployment,
    /SRC_ROOT=\/opt\/dd-next-1[\s\S]*SCRATCH_ROOT=\/tmp\/dd-gleam-mcp-server[\s\S]*WORK_ROOT="\$SCRATCH_ROOT\/dd-next-1"[\s\S]*touch "\$SCRATCH_ROOT\/\.boot-probe"[\s\S]*cp -R "\$SRC_ROOT\/remote\/deployments\/gleam-mcp-server"\/\. "\$WORK_ROOT\/remote\/deployments\/gleam-mcp-server"\/[\s\S]*cp -R "\$SRC_ROOT\/remote\/libs\/cli-config-client-gleam"\/\. "\$WORK_ROOT\/remote\/libs\/cli-config-client-gleam"\/[\s\S]*cp -R "\$SRC_ROOT\/remote\/libs\/pg-defs\/generated\/gleam" "\$WORK_ROOT\/remote\/libs\/pg-defs\/generated\/gleam"[\s\S]*cp -R "\$SRC_ROOT\/remote\/libs\/runtime-config-client-gleam"\/\. "\$WORK_ROOT\/remote\/libs\/runtime-config-client-gleam"\/[\s\S]*cd "\$WORK_ROOT\/remote\/deployments\/gleam-mcp-server"[\s\S]*if \[ ! -f manifest\.toml \][\s\S]*if \[ ! -d build\/packages \][\s\S]*exec gleam run/,
  );
  assert.doesNotMatch(
    ec2Deployment,
    /\n\s+cd \/opt\/dd-next-1\/remote\/deployments\/gleam-mcp-server\n\s+if \[ ! -f manifest\.toml \]/,
  );
  // The boot script must use a deterministic write-probe instead of the
  // older noisy `mountpoint -q` heuristic, and must defensively re-check
  // that build/packages matches every entry in manifest.toml. The exact
  // failure mode this catches: a new local-path dep is added to
  // manifest.toml but the host's build/packages is left stale, so
  // gleam tries to re-resolve over the network and trips the
  // NetworkPolicy hex.pm block — visible end-to-end once as a public
  // gateway 502 with `Resolving versions` in the pod logs.
  assert.doesNotMatch(ec2Deployment, /mountpoint -q\s+"\$SCRATCH_ROOT"/);
  assert.match(
    ec2Deployment,
    /touch "\$SCRATCH_ROOT\/\.boot-probe"[\s\S]*\$SCRATCH_ROOT is not writable/,
  );
  assert.match(
    ec2Deployment,
    /names=\$\(awk '\/\^packages = \\\[\/,\/\^\\\]\/' manifest\.toml[\s\S]*grep -oE 'name = "\[\^"\]\+"'[\s\S]*for name in \$names; do[\s\S]*build\/packages\/\$name\.config_fingerprint[\s\S]*build\/packages stale relative to manifest\.toml/,
  );
  assert.match(
    ec2Deployment,
    /hostPath:\s*\n\s+path:\s*\/home\/ec2-user\/codes\/dd\/dd-next-1[\s\S]*type:\s*Directory/,
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
  assert.match(ec2Deployment, /requests:[\s\S]*cpu:\s*250m[\s\S]*memory:\s*1Gi[\s\S]*ephemeral-storage:\s*256Mi/);
  assert.match(ec2Deployment, /limits:[\s\S]*cpu:\s*"4"[\s\S]*memory:\s*8Gi[\s\S]*ephemeral-storage:\s*2Gi/);
  assert.match(ec2Deployment, /mountPath:\s*\/opt\/dd-next-1/);
  // The hostPath mount must be readOnly so even an accidental write or a
  // misconfigured boot script can't corrupt the source-of-truth host repo
  // checkout under /home/ec2-user/codes/dd/dd-next-1.
  assert.match(
    ec2Deployment,
    /- name: repo\s*\n\s+mountPath: \/opt\/dd-next-1\s*\n\s+readOnly: true/,
  );
  // The scratch space must be an explicit, bounded emptyDir mounted at
  // the exact path the boot script uses (/tmp/dd-gleam-mcp-server) so
  // the boot-time copy never consumes the writable layer unbounded.
  assert.match(
    ec2Deployment,
    /- name: scratch\s*\n\s+mountPath: \/tmp\/dd-gleam-mcp-server/,
  );
  assert.match(
    ec2Deployment,
    /- name: scratch\s*\n\s+emptyDir:\s*\n\s+sizeLimit:\s*1Gi/,
  );
  // The main container runs with a read-only root filesystem. The only
  // mutable surfaces are the two bounded emptyDirs (scratch + tmp), and
  // gleam's implicit writes (cache/home/gleam-home) are env-redirected
  // into the scratch mount. Verified end-to-end against
  // ghcr.io/gleam-lang/gleam:v1.16.0-erlang-alpine on the docker engine.
  assert.match(
    ec2Deployment,
    /- name: gleam-mcp-server[\s\S]*securityContext:[\s\S]*readOnlyRootFilesystem:\s*true/,
  );
  assert.match(
    ec2Deployment,
    /- name: tmp\s*\n\s+mountPath: \/tmp/,
  );
  assert.match(
    ec2Deployment,
    /- name: tmp\s*\n\s+emptyDir:\s*\n\s+sizeLimit:\s*64Mi/,
  );
  assert.match(
    ec2Deployment,
    /- name: HOME\s*\n\s+value: \/tmp\/dd-gleam-mcp-server\/home/,
  );
  assert.match(
    ec2Deployment,
    /- name: XDG_CACHE_HOME\s*\n\s+value: \/tmp\/dd-gleam-mcp-server\/cache/,
  );
  assert.match(
    ec2Deployment,
    /- name: GLEAM_HOME\s*\n\s+value: \/tmp\/dd-gleam-mcp-server\/gleam-home/,
  );
  // Boot script must mkdir those env-pinned subdirs before exec'ing
  // gleam (otherwise gleam will try to create them on the read-only
  // rootfs if HOME/XDG/GLEAM_HOME isn't already there).
  assert.match(
    ec2Deployment,
    /mkdir -p "\$SCRATCH_ROOT\/home" "\$SCRATCH_ROOT\/cache" "\$SCRATCH_ROOT\/gleam-home"/,
  );
  // Init container must run before the main container, fail fast on a
  // bad host checkout, and produce operator-actionable log lines that
  // point at the exact host-side fix (warm the checkout / run
  // `gleam deps download` on the EC2 node). This is the line of defence
  // that turns the "silent CrashLoop -> 502 at the gateway" failure mode
  // into a visible `kubectl describe pod` error.
  assert.match(
    ec2Deployment,
    /initContainers:[\s\S]*- name: preflight[\s\S]*image:\s*ghcr\.io\/gleam-lang\/gleam:v1\.16\.0-erlang-alpine[\s\S]*readOnlyRootFilesystem:\s*true[\s\S]*for path in[\s\S]*\$PROJ\/gleam\.toml[\s\S]*\$PROJ\/manifest\.toml[\s\S]*\/opt\/dd-next-1\/remote\/libs\/cli-config-client-gleam\/gleam\.toml[\s\S]*\/opt\/dd-next-1\/remote\/libs\/pg-defs\/generated\/gleam\/gleam\.toml[\s\S]*\/opt\/dd-next-1\/remote\/libs\/runtime-config-client-gleam\/gleam\.toml[\s\S]*build\/packages is empty in the host checkout[\s\S]*gleam deps download[\s\S]*preflight: ok/,
  );
  // Preflight must also fail fast when the host's build/packages exists
  // but is missing entries for packages listed in manifest.toml (the
  // exact stale-cache failure mode we observed once: a new local-path
  // dep was added but the host was never re-warmed).
  assert.match(
    ec2Deployment,
    /initContainers:[\s\S]*- name: preflight[\s\S]*names=\$\(awk '\/\^packages = \\\[\/,\/\^\\\]\/' "\$PROJ\/manifest\.toml"[\s\S]*grep -oE 'name = "\[\^"\]\+"'[\s\S]*for name in \$names; do[\s\S]*build\/packages\/\$name\.config_fingerprint[\s\S]*build\/packages on the host is stale, missing entries for:[\s\S]*preflight: ok/,
  );
  // The init container reads the host repo but must never write to it.
  assert.match(
    ec2Deployment,
    /initContainers:[\s\S]*- name: preflight[\s\S]*volumeMounts:[\s\S]*- name: repo\s*\n\s+mountPath: \/opt\/dd-next-1\s*\n\s+readOnly: true/,
  );
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
  assert.match(ec2Kustomization, /dd-gleam-mcp-server\.pdb\.yaml/);
  // The single-replica MCP runs with Recreate strategy, so a voluntary
  // node drain would normally take the only pod down with no warning.
  // The PodDisruptionBudget keeps a manual `kubectl drain` honest by
  // requiring an explicit --force / --disable-eviction.
  assert.match(ec2Pdb, /apiVersion:\s*policy\/v1/);
  assert.match(ec2Pdb, /kind:\s*PodDisruptionBudget/);
  assert.match(ec2Pdb, /name:\s*dd-gleam-mcp-server\s*\n/);
  assert.match(ec2Pdb, /namespace:\s*default/);
  assert.match(ec2Pdb, /minAvailable:\s*1/);
  assert.match(ec2Pdb, /selector:[\s\S]*matchLabels:[\s\S]*app:\s*dd-gleam-mcp-server/);
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
  // The preflight init container, the boot script, and this whole pod
  // design assume the NetworkPolicy egress block has no public-internet
  // escape hatch (we cannot reach hex.pm at runtime). Lock that in: no
  // `- ipBlock: cidr: 0.0.0.0/0`, no all-allow `to: []` or `to: {}`.
  assert.doesNotMatch(ec2NetworkPolicy, /0\.0\.0\.0\/0/);
  assert.doesNotMatch(ec2NetworkPolicy, /to:\s*\[\s*\]/);
  assert.doesNotMatch(ec2NetworkPolicy, /to:\s*\{\s*\}/);
  // Warm worker containers spawned by `dd-container-pool` reach MCP from
  // the EC2 node host network, so the policy must allow RFC1918 ingress
  // on :8090 in addition to the labelled podSelector clauses above.
  assert.match(
    ec2NetworkPolicy,
    /ipBlock:\s*\n\s+cidr:\s*10\.0\.0\.0\/8[\s\S]*ipBlock:\s*\n\s+cidr:\s*172\.16\.0\.0\/12[\s\S]*ipBlock:\s*\n\s+cidr:\s*192\.168\.0\.0\/16[\s\S]*ports:[\s\S]*port:\s*8090/,
  );
  // dd-runtime-config wires through env (RUNTIME_CONFIG_REGISTER_URL +
  // RUNTIME_CONFIG_APPLY_URL). Egress on :8110 lets the pod subscribe;
  // ingress on :8090 from the runtime-config pod lets it deliver the
  // apply callback. Without these the pod logs
  // "[runtime-config] register error: timeout; retrying in N s" forever
  // while the rest of MCP works.
  assert.match(
    ec2NetworkPolicy,
    /- to:\s*\n\s+- podSelector:\s*\n\s+matchLabels:\s*\n\s+app:\s*dd-runtime-config\s*\n\s+ports:\s*\n\s+- protocol:\s*TCP\s*\n\s+port:\s*8110/,
  );
  assert.match(
    ec2NetworkPolicy,
    /- from:\s*\n\s+- podSelector:\s*\n\s+matchLabels:\s*\n\s+app:\s*dd-runtime-config\s*\n\s+ports:\s*\n\s+- protocol:\s*TCP\s*\n\s+port:\s*8090/,
  );
  assert.match(ec2App, /path:\s*remote\/deployments\/gleam-mcp-server\/k8s\/ec2/);
  assert.match(ec2Verifier, /^#!\/usr\/bin\/env bash/m);
  assert.match(ec2Verifier, /remote\/argocd\/apps\/dd-gleam-mcp-server\.application\.yaml/);
  assert.match(ec2Verifier, /MCP_EXPECTED_APP_PATH:-remote\/deployments\/gleam-mcp-server\/k8s\/ec2/);
  assert.match(ec2Verifier, /\*kind\*\|\*docker-desktop\*\|\*colima\*/);
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
  // The verifier must also exercise the hardening we added on top of
  // the 502 fix: confirm the preflight init container ran and produced
  // its OK sentinel, and confirm the PDB is in place with minAvailable=1.
  assert.match(ec2Verifier, /kubectl -n "\$\{namespace\}" logs "\$\{pod\}" -c preflight/);
  assert.match(ec2Verifier, /grep -q '\^preflight: ok\$'/);
  assert.match(ec2Verifier, /kubectl -n "\$\{namespace\}" get poddisruptionbudget/);
  assert.match(ec2Verifier, /minAvailable=1/);
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
  assert.match(dashboard, /Cluster MCP Runtime/);
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
  assert.match(readme, /Kubernetes API egress on TCP 443/);
  // The hostPath-cache pattern is fragile in exactly one way (stale
  // build/packages → 502 at the gateway). The readme must document the
  // exact warm-up incantation operators should run on the EC2 node so
  // the next person hitting this regression has a one-shot fix in front
  // of them.
  assert.match(readme, /Warming `build\/packages` on the EC2 host/);
  assert.match(readme, /sudo nerdctl run --rm --net=host[\s\S]*ghcr\.io\/gleam-lang\/gleam:v1\.16\.0-erlang-alpine[\s\S]*gleam deps download/);
  assert.match(readme, /kubectl -n default rollout restart deploy\/dd-gleam-mcp-server/);
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
