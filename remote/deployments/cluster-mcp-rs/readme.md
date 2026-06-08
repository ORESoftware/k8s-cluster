# DD Cluster MCP Rust Server

`cluster-mcp-rs` is the Rust implementation of the `dd_cluster` MCP surface for
the EC2 Kubernetes runtime. It is read-only and intended for AI tools that need
live cluster inventory, service wiring, and observability context before they
guess.

## HTTP Surface

- `GET /home` renders a small service page.
- `GET /healthz` and `GET /readyz` return JSON health.
- `GET /metrics` exposes Prometheus text metrics.
- `GET /observability` returns the same bounded read-only telemetry summary as
  the MCP tool.
- `GET /mcp` returns endpoint metadata.
- `POST /mcp` accepts JSON-RPC MCP requests.

The public gateway exposes this service under `/cluster-mcp` and keeps it
operator-authenticated. Thread agents should use the in-cluster URL:

```text
http://dd-cluster-mcp-rs.default.svc.cluster.local:8091/mcp
```

## MCP Tools

The Rust service preserves the current `dd_cluster` tool catalog:

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

`kubernetes_inventory` uses the service account to list bounded Kubernetes
metadata across namespaces. It excludes Secrets, ConfigMap data, pod logs, exec,
and mutation verbs. `service_directory` augments the static gateway directory
with live Kubernetes Service summaries so agents can see service ports,
selectors, and cluster-local DNS names without reading secret-bearing objects.

## Boundary Hardening

The JSON-RPC HTTP body is capped at 1 MiB by Axum before handler extraction and
again in the MCP handler. Requests must be JSON-RPC 2.0 objects; batch arrays,
empty methods, malformed JSON, and structured/non-scalar ids are rejected or
normalized to a `null` response id.

Environment-driven timeout, body-size, and item-count knobs are clamped in
process. MCP target URL overrides are accepted only for loopback hosts or
cluster service DNS (`*.svc` / `*.svc.cluster.local`) unless
`MCP_ALLOW_EXTERNAL_URLS=true` is set for a deliberate local/operator test.
Returned target URLs are stripped of userinfo, query strings, and fragments.

Kubernetes and observability response samples are redacted before being returned
to MCP clients. JSON bodies are parsed and secret-like keys such as `token`,
`secret`, `password`, `authorization`, `api_key`, and `client_secret` are
replaced with `<redacted>`; plain-text samples get a conservative line-level
fallback.

## Telemetry

The service emits explicit telemetry only:

- Prometheus counters at `/metrics`.
- Compact `dd.log.v1` stdout events for JSON-RPC request and tool-call
  boundaries, collected by Promtail/Loki.
- Best-effort OTLP/HTTP spans to `OTEL_EXPORTER_OTLP_ENDPOINT` when
  `OTEL_TRACES_ENABLED` is not `false`.

It does not monkey-patch Rust, tokio, reqwest, stdout/stderr, module loading, or
any framework internals.

## Kubernetes

Deploy the EC2 Argo CD application:

```sh
kubectl apply -f remote/argocd/apps/dd-cluster-mcp-rs.application.yaml
```

The deployment uses read-only RBAC and runs from the EC2 host checkout with
`cargo run --release --locked`, matching the current Rust runtime pattern. The
checked-in Dockerfile is ready for a later prebuilt image path.
