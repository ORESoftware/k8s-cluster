# DD Gleam MCP Server

This is a standalone Gleam/OTP MCP service for the remote Kubernetes runtime. It runs as its own
Kubernetes Deployment and Argo CD Application, separate from the Rust web UI, Rust REST API,
Node.js coding-agent workers, and the Gleam WebSocket service.

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
- `telemetry_targets`
- `observability_snapshot`

## Telemetry

The service exports:

- `dd_gleam_mcp_http_requests_total`
- `dd_gleam_mcp_rpc_requests_total`

The OpenTelemetry Collector scrapes `dd-gleam-mcp-server.default.svc.cluster.local:8090` and
re-exports the metrics to Prometheus. Logs go to stdout, where promtail collects them for Loki.
Grafana dashboard panels live in `remote/argocd/observability/grafana.dashboards.configmap.yaml`.

The MCP server can also read the observability plane through internal service DNS without routing
through the public gateway. The `observability_snapshot` tool and `GET /observability` probe:

- Prometheus: readiness plus `up` query.
- Loki: readiness plus labels API.
- Grafana: health API.
- OpenTelemetry Collector: collector-exported Prometheus metrics.
- Tempo and Jaeger: trace backend readiness/query APIs.
- NATS metrics exporter: Prometheus metrics endpoint.

Deployment env vars keep those URLs explicit and overrideable: `MCP_PROMETHEUS_URL`,
`MCP_LOKI_URL`, `MCP_GRAFANA_URL`, `MCP_OTEL_COLLECTOR_URL`, `MCP_TEMPO_URL`, `MCP_JAEGER_URL`, and
`MCP_NATS_METRICS_URL`. The snapshot probes targets concurrently and is bounded by
`MCP_OBS_HTTP_TIMEOUT_MS`, `MCP_OBS_SNAPSHOT_TIMEOUT_MS`, and `MCP_OBS_SNIPPET_BYTES`. Responses
return only small body snippets so agents can diagnose telemetry reachability without turning MCP
into a broad data exfiltration path.

## Kubernetes

EC2:

```sh
kubectl apply -f remote/argocd/apps/dd-gleam-mcp-server.application.yaml
```

Minikube:

```sh
kubectl apply -f remote/argocd/apps/dd-gleam-mcp-server-minikube.application.yaml
```
