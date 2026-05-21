# DD Gleam MCP Server

This is a standalone Gleam/OTP MCP service for the EC2 Kubernetes runtime. It runs as its own
Kubernetes Deployment and Argo CD Application on the EC2 cluster, separate from the Rust web UI,
Rust REST API, Node.js coding-agent workers, and the Gleam WebSocket service.

## HTTP Surface

- `GET /home` renders a small service page.
- `GET /healthz` returns JSON health.
- `GET /metrics` returns Prometheus text metrics.
- `GET /observability` returns a bounded read-only snapshot of internal observability endpoints.
- `GET /mcp` returns endpoint metadata.
- `POST /mcp` accepts a minimal JSON-RPC MCP request.

The public gateway exposes these under:

- `https://54.91.17.58/mcp/home`
- `https://54.91.17.58/mcp/healthz`
- `https://54.91.17.58/mcp/metrics`
- `https://54.91.17.58/mcp/observability`
- `https://54.91.17.58/mcp`

Ops paths are currently protected by the temporary dd gateway auth header. Do not echo the
configured value in public responses or docs.

## Secrets

The MCP server should not own raw secrets in Git. If it needs credentials later for write-capable
tools, add those keys to AWS Secrets Manager and expose them through External Secrets Operator:

- ArgoCD app: `remote/argocd/apps/external-secrets-operator.application.yaml`
- Secret manifests: `remote/argocd/secrets/`
- Source of truth: AWS Secrets Manager JSON secrets
- Generated Kubernetes secrets: `dd-agent-secrets`, `dd-remote-rest-api-secrets`, and
  service-specific secrets as needed

For MCP-specific credentials, keep using the dedicated AWS secret
`dd/remote-dev/mcp-secrets`, the matching `ExternalSecret` manifest, and the generated Kubernetes
secret mounted only into the MCP deployment. Keep MCP tools read-only until the auth story is
stronger.

An admin UI can manage these values safely if it never displays secret values: the browser submits
a new value to an authenticated server route, that route writes a new AWS Secrets Manager version,
and ArgoCD/External Secrets handles cluster sync. A GitHub Actions `workflow_dispatch` can do the
same thing with masked inputs and then run an ArgoCD sync, while GitHub stores only manifests, not
secret values.

### MCP Secret-Management Shape

The MCP service is intentionally read-only right now. That is the right default because MCP tools
are easy to wire into agents, and a write-capable secret tool becomes a high-trust administrative
surface.

If we add MCP secret management later, keep it narrow:

- `secrets/list_metadata` may return secret group names, expected key names, ExternalSecret status,
  refresh timestamps, and deployment consumers.
- `secrets/rotate` may accept `{group, key, newValue}` and write a new AWS Secrets Manager version.
- `secrets/sync` may trigger ArgoCD sync for `dd-secrets` and restart selected deployments.
- No MCP method should return current or previous secret values.
- Every write should require admin auth, produce an audit event, and include a task id so retries
  are idempotent.

For MCP-specific credentials, use a dedicated AWS secret and Kubernetes secret:

| Source                      | Generated target              | Deployment access                        |
| --------------------------- | ----------------------------- | ---------------------------------------- |
| `dd/remote-dev/mcp-secrets` | `dd-gleam-mcp-server-secrets` | Mounted only into `dd-gleam-mcp-server`. |

The current MCP secret shape includes `RDS_DATABASE_URL` and `AGENT_TASKS_RDS_DATABASE_URL` so
read-only MCP tools can inspect database-backed contract metadata without inheriting the broader
REST API or agent secret bundles.

Do not share the broad agent model-key secret with MCP unless the MCP tool truly needs to call
model providers. Prefer per-service secrets so a compromised MCP pod cannot automatically inherit
worker tokens, database URLs, or GitHub credentials.

### Admin UI And Automation

The recommended operator experience is:

1. Use a small authenticated admin UI, for example `/agents/secrets`, to submit replacement values.
2. The UI posts to a server route; the browser never receives AWS credentials or existing secret
   values.
3. The server writes a new AWS Secrets Manager version and records a redacted audit event.
4. ArgoCD syncs `dd-secrets`, External Secrets Operator refreshes Kubernetes `Secret` objects, and
   the server restarts only deployments that consume the changed group.

GitHub Actions can provide the same operation for CI-friendly rotations. Use `workflow_dispatch`,
AWS OIDC, masked inputs, and a narrow IAM role limited to `dd/remote-dev/*`. GitHub hooks are
useful for syncing manifests after a merge, but they should not carry raw secret values.

## MCP Methods

The first pass supports the current MCP protocol revision `2025-11-25` and the read-only tools
surface:

- `initialize`
- `notifications/initialized`
- `ping`
- `tools/list`
- `tools/call`

The tools are intentionally read-only:

- `cluster_status`
- `service_directory`
- `kubernetes_inventory`
- `kubernetes_deployments`
- `human_access_policy`
- `telemetry_targets`
- `telemetry_summary`
- `observability_health`
- `prometheus_up`
- `loki_labels`
- `grafana_inventory`
- `nats_metrics`
- `trace_backends`

`kubernetes_inventory` reads bounded `PartialObjectMetadata` snapshots from the in-cluster
Kubernetes API for namespaces, nodes, pods, services, endpoints, PVCs/PVs, service accounts, events,
apps workloads, batch jobs, ingresses, network policies, HPAs, storage classes, and CRDs.
`kubernetes_deployments` remains the narrower all-Deployment view. The shipped RBAC grants only
`list` for the inventory resources. It does not grant Kubernetes Secret access, ConfigMap
data access, pod logs, pod exec, or mutation verbs. The client requests only
`PartialObjectMetadataList`; if the API server cannot serve metadata-only content for a resource, the
tool reports that API error instead of falling back to full objects.

`human_access_policy` describes the human-authenticated gateway/VPN/bastion policy. It never returns
secret values and does not mint elevated grants. The public gateway already requires either the
legacy `Auth` header or the `dd_auth` HttpOnly cookie from `dd-remote-auth`; configure
`DD_AUTH_TOTP_SECRET_BASE32` on that auth service to require passphrase plus a six-digit TOTP code at
the beginning of a browser session.

| Env var | Default |
| --- | --- |
| `MCP_KUBERNETES_API_URL` | `https://kubernetes.default.svc` |
| `MCP_KUBERNETES_DEPLOYMENTS_PATH` | `/apis/apps/v1/deployments?limit=500` |
| `MCP_KUBERNETES_TOKEN_PATH` | `/var/run/secrets/kubernetes.io/serviceaccount/token` |
| `MCP_KUBERNETES_CA_PATH` | `/var/run/secrets/kubernetes.io/serviceaccount/ca.crt` |
| `MCP_KUBERNETES_TIMEOUT_MS` | `1500` |
| `MCP_KUBERNETES_BODY_LIMIT_BYTES` | `262144` |
| `MCP_KUBERNETES_INVENTORY_BODY_LIMIT_BYTES` | `32768` |

`telemetry_summary`, `observability_health`, `prometheus_up`, `loki_labels`,
`grafana_inventory`, `nats_metrics`, and `trace_backends` make bounded
in-cluster HTTP reads from the observability and messaging planes. Summary and
health calls fan out checks in parallel so MCP clients do not wait for slow
endpoints one by one, and timed-out checks are returned explicitly in the
structured response. Grafana datasource and dashboard inventory is read through
the anonymous in-cluster Grafana API. They use the service DNS names below by
default and can be overridden per deployment:

| Env var | Default |
| --- | --- |
| `MCP_PROMETHEUS_URL` | `http://dd-prometheus.observability.svc.cluster.local:9090` |
| `MCP_LOKI_URL` | `http://dd-loki.observability.svc.cluster.local:3100` |
| `MCP_GRAFANA_URL` | `http://dd-grafana.observability.svc.cluster.local:3000` |
| `MCP_TEMPO_URL` | `http://dd-tempo.observability.svc.cluster.local:3200` |
| `MCP_JAEGER_URL` | `http://dd-jaeger.observability.svc.cluster.local:16686` |
| `MCP_OTEL_COLLECTOR_URL` | `http://dd-otel-collector.observability.svc.cluster.local:8889` |
| `MCP_NATS_MONITOR_URL` | `http://dd-nats.messaging.svc.cluster.local:8222` |
| `MCP_NATS_METRICS_URL` | `http://dd-nats.messaging.svc.cluster.local:7777` |
| `MCP_OBSERVABILITY_TIMEOUT_MS` | `1200` |
| `MCP_OBSERVABILITY_BODY_LIMIT_BYTES` | `32768` |

## Telemetry

The service exports:

- `dd_gleam_mcp_http_requests_total`
- `dd_gleam_mcp_rpc_requests_total`

The OpenTelemetry Collector scrapes `dd-gleam-mcp-server.default.svc.cluster.local:8090` and
re-exports the metrics to Prometheus. Logs go to stdout, where promtail collects them for Loki.
Grafana dashboard panels live in `remote/argocd/observability/grafana.dashboards.configmap.yaml`.

The MCP server reads observability data directly from the in-cluster
Prometheus/Loki/Grafana/Tempo/Jaeger/OTel services and from the NATS monitoring
and metrics endpoints without Kubernetes API permissions for that telemetry path.
The Deployment also has a read-only service account for Deployment inventory.
It does not expose write-capable telemetry, Kubernetes, AWS, or secret-management
operations. The deployment includes a NetworkPolicy that permits ingress from the
gateway, the dev-server supervisor (`app: dd-dev-server-api`), per-thread agent
worker pods (`app.kubernetes.io/part-of: dd-remote-dev` +
`app.kubernetes.io/component: thread-pod`), and metrics scrapers in the
`observability` namespace; DNS egress, bounded egress to observability and NATS
telemetry ports, Kubernetes API egress on TCP 443, and database egress for
future read-only PG-backed MCP tools.

If you add a new in-cluster MCP caller, give its pod template one of those
labels (or extend the NetworkPolicy ingress in
`remote/deployments/gleam-mcp-server/k8s/ec2/dd-gleam-mcp-server.networkpolicy.yaml`).
The most common symptom of a missing entry is the OpenAI Agents SDK runner
emitting `openai-sdk: MCP server dd_cluster unavailable at
http://dd-gleam-mcp-server.default.svc.cluster.local:8090/mcp` because the TCP
SYN is dropped at the CNI before reaching the MCP pod.

## Kubernetes

EC2 is the production/canonical target for MCP:

```sh
kubectl apply -f remote/argocd/apps/dd-gleam-mcp-server.application.yaml
```

That Argo CD application must point at `remote/deployments/gleam-mcp-server/k8s/ec2` and deploy to namespace
`default`. From the EC2 host, or from any shell whose kubeconfig points at the EC2 cluster, run:

```sh
remote/ec2/verify-gleam-mcp-server.sh
```

The verifier refuses obvious local contexts such as `minikube` unless
`ALLOW_NON_EC2_CONTEXT=true` is set explicitly. It applies the EC2 Argo app, checks that the app
tracks the EC2 overlay, validates the read-only RBAC shape, waits for rollout, and calls
`human_access_policy` plus `kubernetes_inventory` through the in-cluster service.

The minikube overlay is only a local mirror for development/rendering. Do not use it for live MCP
cluster context, agent runtime defaults, or gateway-facing MCP work.
