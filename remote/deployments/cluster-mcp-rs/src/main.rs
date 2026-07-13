use std::{
    borrow::Cow,
    collections::BTreeMap,
    env,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    body::Bytes,
    extract::{ConnectInfo, DefaultBodyLimit, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use futures_util::{stream::FuturesUnordered, StreamExt};
use rand::RngCore;
use reqwest::Certificate;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use subtle::ConstantTimeEq;

const SERVICE_NAME: &str = "dd-cluster-mcp-rs";
const SERVICE_NAMESPACE: &str = "default";
const SERVICE_VERSION: &str = "0.1.0";
const SERVICE_SCOPE: &str = "cluster-mcp-rs";
const RESOURCE_NAMESPACE: &str = "remote-dev";
const PROTOCOL_VERSION: &str = "2025-11-25";
// Newest first. `initialize` echoes the client's requested protocolVersion
// when it is one of these; anything else gets the newest supported version.
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];
const DEFAULT_PORT: u16 = 8091;
const MAX_RPC_BODY_BYTES: usize = 1_000_000;
const MAX_TIMEOUT_MS: u64 = 5_000;
const MAX_KUBERNETES_BODY_LIMIT_BYTES: usize = 262_144;
const MAX_KUBERNETES_ITEMS_LIMIT: usize = 500;
const MAX_OBSERVABILITY_BODY_LIMIT_BYTES: usize = 262_144;
const REDACTED: &str = "<redacted>";

const TOOL_NAMES: &[&str] = &[
    "cluster_status",
    "service_directory",
    "kubernetes_inventory",
    "kubernetes_deployments",
    "human_access_policy",
    "telemetry_targets",
    "telemetry_summary",
    "observability_health",
    "prometheus_up",
    "loki_labels",
    "grafana_inventory",
    "nats_metrics",
    "trace_backends",
];

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    http: reqwest::Client,
    k8s_http: reqwest::Client,
    metrics: Arc<Metrics>,
}

#[derive(Clone)]
struct Config {
    host: String,
    port: u16,
    kubernetes_api_url: String,
    kubernetes_token_path: String,
    kubernetes_ca_path: String,
    kubernetes_timeout: Duration,
    kubernetes_body_limit_bytes: usize,
    kubernetes_inventory_body_limit_bytes: usize,
    kubernetes_items_limit: usize,
    prometheus_url: String,
    loki_url: String,
    grafana_url: String,
    tempo_url: String,
    jaeger_url: String,
    otel_collector_metrics_url: String,
    nats_monitor_url: String,
    nats_metrics_url: String,
    observability_timeout: Duration,
    observability_body_limit_bytes: usize,
    otlp_endpoint: Option<String>,
    otlp_timeout: Duration,
    require_auth: bool,
    auth_secret: Option<String>,
}

#[derive(Default)]
struct Metrics {
    http_requests_total: AtomicU64,
    rpc_requests_total: AtomicU64,
    rpc_errors_total: AtomicU64,
    tool_calls_total: AtomicU64,
    tool_failures_total: AtomicU64,
    k8s_requests_total: AtomicU64,
    observability_requests_total: AtomicU64,
    otlp_spans_total: AtomicU64,
    otlp_failures_total: AtomicU64,
    rpc_by_method: Mutex<BTreeMap<String, u64>>,
    tool_by_name: Mutex<BTreeMap<String, u64>>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: Option<String>,
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

#[derive(Debug, Clone, Copy)]
struct ResourceTarget {
    name: &'static str,
    scope: &'static str,
    path: &'static str,
}

#[derive(Debug, Clone)]
struct TraceContext {
    trace_id: String,
    span_id: String,
    start_unix_nano: u128,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthResponse {
    ok: bool,
    service: &'static str,
    namespace: &'static str,
    protocol_version: &'static str,
    tools: &'static [&'static str],
    otlp_enabled: bool,
}

fn env_string(key: &str, fallback: &str) -> String {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn env_bool(key: &str, fallback: bool) -> bool {
    env::var(key)
        .ok()
        .map(|value| {
            matches!(
                value.as_str(),
                "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
            )
        })
        .unwrap_or(fallback)
}

fn env_u64_bounded(key: &str, fallback: u64, min: u64, max: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value >= min && *value <= max)
        .unwrap_or(fallback)
}

fn env_usize_bounded(key: &str, fallback: usize, min: usize, max: usize) -> usize {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value >= min && *value <= max)
        .unwrap_or(fallback)
}

fn env_mcp_base_url(key: &str, fallback: &str, allow_external: bool) -> String {
    let value = env_string(key, fallback);
    if allow_external || allowed_mcp_base_url(&value) {
        value
    } else {
        fallback.to_string()
    }
}

fn allowed_mcp_base_url(raw: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(raw) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1") {
        return true;
    }
    host == "kubernetes.default.svc"
        || host.ends_with(".svc")
        || host.ends_with(".svc.cluster.local")
}

fn config_from_env() -> Config {
    let allow_external_mcp_urls = env_bool("MCP_ALLOW_EXTERNAL_URLS", false);
    let otlp_endpoint = if env_bool("OTEL_TRACES_ENABLED", true) {
        env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(|value| {
                if value.ends_with("/v1/traces") {
                    value
                } else {
                    format!("{}/v1/traces", value.trim_end_matches('/'))
                }
            })
    } else {
        None
    };

    Config {
        host: env_string("HOST", "0.0.0.0"),
        port: env_u64_bounded("PORT", DEFAULT_PORT as u64, 1, u16::MAX as u64) as u16,
        kubernetes_api_url: env_mcp_base_url(
            "MCP_KUBERNETES_API_URL",
            "https://kubernetes.default.svc",
            allow_external_mcp_urls,
        ),
        kubernetes_token_path: env_string(
            "MCP_KUBERNETES_TOKEN_PATH",
            "/var/run/secrets/kubernetes.io/serviceaccount/token",
        ),
        kubernetes_ca_path: env_string(
            "MCP_KUBERNETES_CA_PATH",
            "/var/run/secrets/kubernetes.io/serviceaccount/ca.crt",
        ),
        kubernetes_timeout: Duration::from_millis(env_u64_bounded(
            "MCP_KUBERNETES_TIMEOUT_MS",
            1500,
            100,
            MAX_TIMEOUT_MS,
        )),
        kubernetes_body_limit_bytes: env_usize_bounded(
            "MCP_KUBERNETES_BODY_LIMIT_BYTES",
            262_144,
            1024,
            MAX_KUBERNETES_BODY_LIMIT_BYTES,
        ),
        kubernetes_inventory_body_limit_bytes: env_usize_bounded(
            "MCP_KUBERNETES_INVENTORY_BODY_LIMIT_BYTES",
            32_768,
            1024,
            MAX_KUBERNETES_BODY_LIMIT_BYTES,
        ),
        kubernetes_items_limit: env_usize_bounded(
            "MCP_KUBERNETES_ITEMS_LIMIT",
            200,
            1,
            MAX_KUBERNETES_ITEMS_LIMIT,
        ),
        prometheus_url: env_mcp_base_url(
            "MCP_PROMETHEUS_URL",
            "http://dd-prometheus.observability.svc.cluster.local:9090",
            allow_external_mcp_urls,
        ),
        loki_url: env_mcp_base_url(
            "MCP_LOKI_URL",
            "http://dd-loki.observability.svc.cluster.local:3100",
            allow_external_mcp_urls,
        ),
        grafana_url: env_mcp_base_url(
            "MCP_GRAFANA_URL",
            "http://dd-grafana.observability.svc.cluster.local:3000",
            allow_external_mcp_urls,
        ),
        tempo_url: env_mcp_base_url(
            "MCP_TEMPO_URL",
            "http://dd-tempo.observability.svc.cluster.local:3200",
            allow_external_mcp_urls,
        ),
        jaeger_url: env_mcp_base_url(
            "MCP_JAEGER_URL",
            "http://dd-jaeger.observability.svc.cluster.local:16686",
            allow_external_mcp_urls,
        ),
        otel_collector_metrics_url: env_mcp_base_url(
            "MCP_OTEL_COLLECTOR_URL",
            "http://dd-otel-collector.observability.svc.cluster.local:8889",
            allow_external_mcp_urls,
        ),
        nats_monitor_url: env_mcp_base_url(
            "MCP_NATS_MONITOR_URL",
            "http://dd-nats.messaging.svc.cluster.local:8222",
            allow_external_mcp_urls,
        ),
        nats_metrics_url: env_mcp_base_url(
            "MCP_NATS_METRICS_URL",
            "http://dd-nats.messaging.svc.cluster.local:7777",
            allow_external_mcp_urls,
        ),
        observability_timeout: Duration::from_millis(env_u64_bounded(
            "MCP_OBSERVABILITY_TIMEOUT_MS",
            1200,
            100,
            MAX_TIMEOUT_MS,
        )),
        observability_body_limit_bytes: env_usize_bounded(
            "MCP_OBSERVABILITY_BODY_LIMIT_BYTES",
            32_768,
            1024,
            MAX_OBSERVABILITY_BODY_LIMIT_BYTES,
        ),
        otlp_endpoint,
        otlp_timeout: Duration::from_millis(env_u64_bounded(
            "OTEL_EXPORT_TIMEOUT_MS",
            800,
            100,
            MAX_TIMEOUT_MS,
        )),
        require_auth: env_bool("MCP_REQUIRE_AUTH", false),
        auth_secret: env::var("MCP_AUTH_SECRET")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
    }
}

// Optional bearer gate on POST /mcp, mirroring the Gleam server. Off by default
// so the gateway-enforced auth model is unchanged; when MCP_REQUIRE_AUTH is set
// the pod also requires `Authorization: Bearer <MCP_AUTH_SECRET>` or
// `X-Server-Auth: <secret>`, which lets an operator close the unauthenticated
// in-VPC ingress path without rewriting the NetworkPolicy. Fails closed if
// required but no secret is configured.
fn request_authorized(config: &Config, headers: &HeaderMap) -> bool {
    header_secret_ok(config.auth_secret.as_deref(), headers)
}

fn header_secret_ok(secret: Option<&str>, headers: &HeaderMap) -> bool {
    let Some(secret) = secret.filter(|value| !value.is_empty()) else {
        return false;
    };
    let bearer_ok = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|value| constant_time_secret_eq(secret, value));
    let header_ok = headers
        .get("x-server-auth")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| constant_time_secret_eq(secret, value));
    bearer_ok || header_ok
}

// Constant-time secret comparison so response timing does not reveal how much
// of a guessed secret matched. `subtle` is already in the dependency tree via
// rustls; its slice ct_eq compares every byte of equal-length inputs without
// data-dependent branches (length itself is not secret).
fn constant_time_secret_eq(expected: &str, provided: &str) -> bool {
    expected.as_bytes().ct_eq(provided.as_bytes()).into()
}

// When the bearer gate is on it must also cover the read-only GET surfaces
// that leak cluster state without a tool call: /observability returns the full
// telemetry_summary tool output, /metrics exposes internal counters, and the
// generated API docs describe the deployment. /healthz and /readyz stay open
// so kubelet probes keep working without a secret mount.
fn gated_unauthorized_response(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    if !state.config.require_auth || request_authorized(&state.config, headers) {
        return None;
    }
    let mut response = json_response(
        StatusCode::UNAUTHORIZED,
        json!({ "ok": false, "error": "unauthorized" }),
    );
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Bearer realm=\"dd-cluster-mcp-rs\""),
    );
    Some(response)
}

fn build_k8s_client(config: &Config) -> reqwest::Client {
    // The SA bearer token is sent on these requests, so trust is pinned to the
    // in-cluster CA only (tls_built_in_root_certs(false) drops the public webpki
    // roots — no public CA should be able to vouch for kubernetes.default.svc),
    // and redirects are disabled so a 3xx cannot retarget a token-bearing call
    // to another host. A missing/invalid CA fails closed (TLS rejects), but we
    // log it because the failure is otherwise opaque.
    let mut builder = reqwest::Client::builder()
        .user_agent(format!("{SERVICE_NAME}/{SERVICE_VERSION}"))
        .timeout(config.kubernetes_timeout)
        .redirect(reqwest::redirect::Policy::none())
        .tls_built_in_root_certs(false);
    match std::fs::read(&config.kubernetes_ca_path) {
        Ok(bytes) => match Certificate::from_pem(&bytes) {
            Ok(cert) => builder = builder.add_root_certificate(cert),
            Err(error) => log_dd_event(
                "WARN",
                13,
                "failed to parse Kubernetes service-account CA; k8s tools will fail TLS",
                "cluster_mcp.k8s.ca_parse_failed",
                json!({ "error": error.to_string(), "path": config.kubernetes_ca_path }),
                None,
            ),
        },
        Err(error) => log_dd_event(
            "WARN",
            13,
            "failed to read Kubernetes service-account CA; k8s tools will fail TLS",
            "cluster_mcp.k8s.ca_read_failed",
            json!({ "error": error.to_string(), "path": config.kubernetes_ca_path }),
            None,
        ),
    }
    builder.build().expect("failed to build k8s http client")
}

fn build_http_client(config: &Config) -> reqwest::Client {
    // Observability/OTLP fan-out. Redirects disabled so a compromised in-cluster
    // backend cannot bounce a read to an arbitrary host (SSRF amplifier).
    reqwest::Client::builder()
        .user_agent(format!("{SERVICE_NAME}/{SERVICE_VERSION}"))
        .timeout(config.observability_timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("failed to build http client")
}

fn now_unix_nano() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn elapsed_ms(started: Instant) -> u128 {
    started.elapsed().as_millis()
}

fn random_hex(bytes_len: usize) -> String {
    let mut bytes = vec![0u8; bytes_len];
    rand::thread_rng().fill_bytes(&mut bytes);
    if bytes.iter().all(|byte| *byte == 0) {
        bytes[0] = 1;
    }
    hex::encode(bytes)
}

fn new_trace_context() -> TraceContext {
    TraceContext {
        trace_id: random_hex(16),
        span_id: random_hex(8),
        start_unix_nano: now_unix_nano(),
    }
}

// Source attribution for rpc/tool log events so unauthenticated in-VPC calls
// are traceable to a peer. The socket peer address is ground truth;
// X-Forwarded-For is caller-controlled, so it is logged only as a separate,
// clearly-labelled field (clipped and sanitized, never trusted).
fn client_attrs(attributes: Value, peer: SocketAddr, headers: &HeaderMap) -> Value {
    let mut map = match attributes {
        Value::Object(map) => map,
        other => {
            let mut map = Map::new();
            map.insert("attributes".to_string(), other);
            map
        }
    };
    map.insert("client.ip".to_string(), json!(peer.ip().to_string()));
    if let Some(forwarded) = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .map(|value| sanitize_text(&clip_string(value.trim(), 256)))
        .filter(|value| !value.is_empty())
    {
        map.insert("client.forwarded_for".to_string(), json!(forwarded));
    }
    Value::Object(map)
}

const KNOWN_METHODS: &[&str] = &[
    "initialize",
    "notifications/initialized",
    "ping",
    "tools/list",
    "tools/call",
];

// The per-method / per-tool metric maps are labelled by request-supplied
// strings. Recording them verbatim would let any caller (including the
// unauthenticated in-VPC ingress path) grow the maps without bound — a memory
// exhaustion + /metrics blow-up DoS, and unbounded Prometheus label
// cardinality. Bucket anything outside the fixed known set under "other".
fn bounded_method_label(method: &str) -> &str {
    if KNOWN_METHODS.contains(&method) {
        method
    } else {
        "other"
    }
}

fn bounded_tool_label(tool: &str) -> &str {
    if TOOL_NAMES.contains(&tool) {
        tool
    } else {
        "other"
    }
}

impl Metrics {
    fn record_rpc(&self, method: &str) {
        self.rpc_requests_total.fetch_add(1, Ordering::Relaxed);
        increment_map(&self.rpc_by_method, bounded_method_label(method));
    }

    fn record_tool(&self, tool: &str) {
        self.tool_calls_total.fetch_add(1, Ordering::Relaxed);
        increment_map(&self.tool_by_name, bounded_tool_label(tool));
    }
}

fn increment_map(map: &Mutex<BTreeMap<String, u64>>, key: &str) {
    if let Ok(mut guard) = map.lock() {
        *guard.entry(key.to_string()).or_insert(0) += 1;
    }
}

fn log_dd_event(
    severity_text: &str,
    severity_number: u8,
    body: &str,
    event_name: &str,
    attributes: Value,
    trace: Option<&TraceContext>,
) {
    let mut record = Map::new();
    record.insert("schema".to_string(), json!("dd.log.v1"));
    record.insert(
        "time_unix_nano".to_string(),
        json!(now_unix_nano().to_string()),
    );
    record.insert("severity_text".to_string(), json!(severity_text));
    record.insert("severity_number".to_string(), json!(severity_number));
    record.insert("body".to_string(), json!(body));
    record.insert("resource_service_name".to_string(), json!(SERVICE_NAME));
    record.insert(
        "resource_service_namespace".to_string(),
        json!(RESOURCE_NAMESPACE),
    );
    record.insert("scope_name".to_string(), json!(SERVICE_SCOPE));
    record.insert("event_name".to_string(), json!(event_name));
    if let Some(trace) = trace {
        record.insert("trace_id".to_string(), json!(trace.trace_id));
        record.insert("span_id".to_string(), json!(trace.span_id));
    }
    record.insert("attributes".to_string(), attributes);
    // This function IS the structured-log emitter: it prints an OTLP-style JSON
    // record (already trace-correlated) straight to stdout for promtail/Loki.
    // Keep it as a raw stdout write — routing it through `tracing` would nest
    // JSON inside a tracing message field.
    println!("{}", Value::Object(record));
}

fn finish_span(
    state: AppState,
    trace: TraceContext,
    name: &'static str,
    ok: bool,
    attributes: Value,
) {
    let end_unix_nano = now_unix_nano();
    tokio::spawn(async move {
        emit_otlp_span(&state, &trace, name, ok, attributes, end_unix_nano).await;
    });
}

async fn emit_otlp_span(
    state: &AppState,
    trace: &TraceContext,
    name: &str,
    ok: bool,
    attributes: Value,
    end_unix_nano: u128,
) {
    let Some(endpoint) = state.config.otlp_endpoint.as_deref() else {
        return;
    };
    state
        .metrics
        .otlp_spans_total
        .fetch_add(1, Ordering::Relaxed);
    let attrs = otlp_attributes(attributes);
    let payload = json!({
        "resourceSpans": [{
            "resource": {
                "attributes": [
                    otlp_attr("service.name", SERVICE_NAME),
                    otlp_attr("service.namespace", RESOURCE_NAMESPACE),
                    otlp_attr("service.version", SERVICE_VERSION),
                    otlp_attr("deployment.environment.name", "stage")
                ]
            },
            "scopeSpans": [{
                "scope": { "name": SERVICE_SCOPE, "version": SERVICE_VERSION },
                "spans": [{
                    "traceId": trace.trace_id,
                    "spanId": trace.span_id,
                    "name": name,
                    "kind": 2,
                    "startTimeUnixNano": trace.start_unix_nano.to_string(),
                    "endTimeUnixNano": end_unix_nano.to_string(),
                    "attributes": attrs,
                    "status": { "code": if ok { 1 } else { 2 } }
                }]
            }]
        }]
    });
    let result = state
        .http
        .post(endpoint)
        .timeout(state.config.otlp_timeout)
        .header(header::CONTENT_TYPE, "application/json")
        .json(&payload)
        .send()
        .await;
    match result {
        Ok(response) if response.status().is_success() => {}
        Ok(response) => {
            state
                .metrics
                .otlp_failures_total
                .fetch_add(1, Ordering::Relaxed);
            log_dd_event(
                "WARN",
                13,
                "OTLP span export returned non-success",
                "cluster_mcp.otlp.export_failed",
                json!({ "status": response.status().as_u16() }),
                Some(trace),
            );
        }
        Err(error) => {
            state
                .metrics
                .otlp_failures_total
                .fetch_add(1, Ordering::Relaxed);
            log_dd_event(
                "WARN",
                13,
                "OTLP span export failed",
                "cluster_mcp.otlp.export_failed",
                json!({ "error": error.to_string() }),
                Some(trace),
            );
        }
    }
}

fn otlp_attr(key: &str, value: &str) -> Value {
    json!({ "key": key, "value": { "stringValue": value } })
}

fn otlp_attributes(attributes: Value) -> Vec<Value> {
    let mut output = Vec::new();
    if let Value::Object(map) = attributes {
        for (key, value) in map {
            let otlp_value = match value {
                Value::Bool(value) => json!({ "boolValue": value }),
                Value::Number(number) => {
                    if let Some(value) = number.as_i64() {
                        json!({ "intValue": value.to_string() })
                    } else if let Some(value) = number.as_u64() {
                        json!({ "intValue": value.to_string() })
                    } else if let Some(value) = number.as_f64() {
                        json!({ "doubleValue": value })
                    } else {
                        json!({ "stringValue": number.to_string() })
                    }
                }
                Value::String(value) => json!({ "stringValue": value }),
                other => json!({ "stringValue": other.to_string() }),
            };
            output.push(json!({ "key": key, "value": otlp_value }));
        }
    }
    output
}

fn id_value(request: &JsonRpcRequest) -> Value {
    sanitize_rpc_id(request.id.as_ref())
}

fn sanitize_rpc_id(id: Option<&Value>) -> Value {
    match id {
        Some(Value::String(_)) | Some(Value::Number(_)) | Some(Value::Null) => {
            id.cloned().unwrap_or(Value::Null)
        }
        _ => Value::Null,
    }
}

fn json_response(status: StatusCode, payload: Value) -> Response {
    let mut response = (status, Json(payload)).into_response();
    response.headers_mut().insert(
        "mcp-protocol-version",
        HeaderValue::from_static(PROTOCOL_VERSION),
    );
    response
}

fn empty_response(status: StatusCode) -> Response {
    let mut response = status.into_response();
    response.headers_mut().insert(
        "mcp-protocol-version",
        HeaderValue::from_static(PROTOCOL_VERSION),
    );
    response
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message
        }
    })
}

async fn root() -> Response {
    (
        StatusCode::FOUND,
        [(header::LOCATION, HeaderValue::from_static("/home"))],
    )
        .into_response()
}

async fn home() -> Html<&'static str> {
    Html(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>dd cluster MCP Rust server</title>
    <style>
      body { margin: 24px; max-width: 880px; font-family: system-ui, sans-serif; line-height: 1.5; color: #142026; }
      code, pre { background: #0f1720; color: #d7fbf4; border-radius: 6px; }
      code { padding: 2px 5px; }
      pre { padding: 12px; overflow: auto; }
      a { color: #047857; }
    </style>
  </head>
  <body>
    <h1>dd cluster MCP Rust server</h1>
    <p>Read-only MCP endpoint for Kubernetes inventory, service discovery, and observability.</p>
    <ul>
      <li>JSON-RPC endpoint: <code>/mcp</code></li>
      <li>Health: <code>/healthz</code></li>
      <li>Prometheus metrics: <code>/metrics</code></li>
      <li>Telemetry summary: <code>/observability</code></li>
    </ul>
    <pre>{"jsonrpc":"2.0","id":1,"method":"tools/list"}</pre>
  </body>
</html>"#,
    )
}

async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(HealthResponse {
        ok: true,
        service: SERVICE_NAME,
        namespace: SERVICE_NAMESPACE,
        protocol_version: PROTOCOL_VERSION,
        tools: TOOL_NAMES,
        otlp_enabled: state.config.otlp_endpoint.is_some(),
    })
}

async fn metrics(State(state): State<AppState>, headers: HeaderMap) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Some(response) = gated_unauthorized_response(&state, &headers) {
        return response;
    }
    let mut body = format!(
        concat!(
            "# HELP dd_cluster_mcp_rs_build_info Cluster MCP Rust build metadata.\n",
            "# TYPE dd_cluster_mcp_rs_build_info gauge\n",
            "dd_cluster_mcp_rs_build_info{{service=\"{service}\",version=\"{version}\"}} 1\n",
            "# HELP dd_cluster_mcp_rs_http_requests_total HTTP requests handled by the Rust MCP server.\n",
            "# TYPE dd_cluster_mcp_rs_http_requests_total counter\n",
            "dd_cluster_mcp_rs_http_requests_total {http}\n",
            "# HELP dd_cluster_mcp_rs_rpc_requests_total JSON-RPC requests handled by the Rust MCP server.\n",
            "# TYPE dd_cluster_mcp_rs_rpc_requests_total counter\n",
            "dd_cluster_mcp_rs_rpc_requests_total {rpc}\n",
            "# HELP dd_cluster_mcp_rs_rpc_errors_total JSON-RPC errors returned by the Rust MCP server.\n",
            "# TYPE dd_cluster_mcp_rs_rpc_errors_total counter\n",
            "dd_cluster_mcp_rs_rpc_errors_total {rpc_errors}\n",
            "# HELP dd_cluster_mcp_rs_tool_calls_total MCP tool calls handled by the Rust MCP server.\n",
            "# TYPE dd_cluster_mcp_rs_tool_calls_total counter\n",
            "dd_cluster_mcp_rs_tool_calls_total {tools}\n",
            "# HELP dd_cluster_mcp_rs_tool_failures_total MCP tool calls that failed before producing a result.\n",
            "# TYPE dd_cluster_mcp_rs_tool_failures_total counter\n",
            "dd_cluster_mcp_rs_tool_failures_total {tool_failures}\n",
            "# HELP dd_cluster_mcp_rs_k8s_requests_total Kubernetes API reads attempted by the Rust MCP server.\n",
            "# TYPE dd_cluster_mcp_rs_k8s_requests_total counter\n",
            "dd_cluster_mcp_rs_k8s_requests_total {k8s}\n",
            "# HELP dd_cluster_mcp_rs_observability_requests_total Observability HTTP reads attempted by the Rust MCP server.\n",
            "# TYPE dd_cluster_mcp_rs_observability_requests_total counter\n",
            "dd_cluster_mcp_rs_observability_requests_total {obs}\n",
            "# HELP dd_cluster_mcp_rs_otlp_spans_total OTLP spans attempted by the Rust MCP server.\n",
            "# TYPE dd_cluster_mcp_rs_otlp_spans_total counter\n",
            "dd_cluster_mcp_rs_otlp_spans_total {otlp}\n",
            "# HELP dd_cluster_mcp_rs_otlp_failures_total OTLP span exports that failed.\n",
            "# TYPE dd_cluster_mcp_rs_otlp_failures_total counter\n",
            "dd_cluster_mcp_rs_otlp_failures_total {otlp_failures}\n"
        ),
        service = SERVICE_NAME,
        version = SERVICE_VERSION,
        http = state.metrics.http_requests_total.load(Ordering::Relaxed),
        rpc = state.metrics.rpc_requests_total.load(Ordering::Relaxed),
        rpc_errors = state.metrics.rpc_errors_total.load(Ordering::Relaxed),
        tools = state.metrics.tool_calls_total.load(Ordering::Relaxed),
        tool_failures = state.metrics.tool_failures_total.load(Ordering::Relaxed),
        k8s = state.metrics.k8s_requests_total.load(Ordering::Relaxed),
        obs = state
            .metrics
            .observability_requests_total
            .load(Ordering::Relaxed),
        otlp = state.metrics.otlp_spans_total.load(Ordering::Relaxed),
        otlp_failures = state.metrics.otlp_failures_total.load(Ordering::Relaxed)
    );

    if let Ok(map) = state.metrics.rpc_by_method.lock() {
        body.push_str(
            "# HELP dd_cluster_mcp_rs_rpc_requests_by_method_total JSON-RPC requests by method.\n",
        );
        body.push_str("# TYPE dd_cluster_mcp_rs_rpc_requests_by_method_total counter\n");
        for (method, value) in map.iter() {
            body.push_str(&format!(
                "dd_cluster_mcp_rs_rpc_requests_by_method_total{{method=\"{}\"}} {}\n",
                prometheus_escape(method),
                value
            ));
        }
    }
    if let Ok(map) = state.metrics.tool_by_name.lock() {
        body.push_str(
            "# HELP dd_cluster_mcp_rs_tool_calls_by_name_total MCP tool calls by tool name.\n",
        );
        body.push_str("# TYPE dd_cluster_mcp_rs_tool_calls_by_name_total counter\n");
        for (tool, value) in map.iter() {
            body.push_str(&format!(
                "dd_cluster_mcp_rs_tool_calls_by_name_total{{tool=\"{}\"}} {}\n",
                prometheus_escape(tool),
                value
            ));
        }
    }

    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

fn prometheus_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

async fn mcp_get(headers: HeaderMap) -> Response {
    if headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains("text/event-stream"))
    {
        return empty_response(StatusCode::METHOD_NOT_ALLOWED);
    }

    json_response(
        StatusCode::OK,
        json!({
            "ok": true,
            "service": SERVICE_NAME,
            "protocolVersion": PROTOCOL_VERSION,
            "endpoint": "POST /mcp",
            "tools": TOOL_NAMES
        }),
    )
}

async fn observability(State(state): State<AppState>) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    json_response(StatusCode::OK, telemetry_summary_json(&state).await)
}

async fn rpc(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if state.config.require_auth && !request_authorized(&state.config, &headers) {
        state
            .metrics
            .rpc_errors_total
            .fetch_add(1, Ordering::Relaxed);
        let mut response = json_response(
            StatusCode::UNAUTHORIZED,
            rpc_error(Value::Null, -32001, "unauthorized"),
        );
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Bearer realm=\"dd-cluster-mcp-rs\""),
        );
        return response;
    }
    if body.len() > MAX_RPC_BODY_BYTES {
        state
            .metrics
            .rpc_errors_total
            .fetch_add(1, Ordering::Relaxed);
        return json_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            rpc_error(Value::Null, -32600, "request body too large"),
        );
    }

    let trace = new_trace_context();
    let request = match serde_json::from_slice::<JsonRpcRequest>(&body) {
        Ok(request) => request,
        Err(error) => {
            state
                .metrics
                .rpc_errors_total
                .fetch_add(1, Ordering::Relaxed);
            log_dd_event(
                "WARN",
                13,
                "MCP JSON-RPC parse error",
                "cluster_mcp.rpc.parse_error",
                json!({ "error": error.to_string() }),
                Some(&trace),
            );
            finish_span(
                state.clone(),
                trace,
                "mcp.rpc.parse_error",
                false,
                json!({ "rpc.method": "parse_error" }),
            );
            return json_response(
                StatusCode::BAD_REQUEST,
                rpc_error(Value::Null, -32700, "parse error"),
            );
        }
    };

    let id = id_value(&request);
    if request.jsonrpc.as_deref() != Some("2.0") || request.method.trim().is_empty() {
        state
            .metrics
            .rpc_errors_total
            .fetch_add(1, Ordering::Relaxed);
        log_dd_event(
            "WARN",
            13,
            "MCP JSON-RPC invalid request",
            "cluster_mcp.rpc.invalid_request",
            json!({ "rpc.method": sanitize_text(&request.method) }),
            Some(&trace),
        );
        finish_span(
            state,
            trace,
            "mcp.rpc.invalid_request",
            false,
            json!({ "rpc.method": sanitize_text(&request.method) }),
        );
        return json_response(
            StatusCode::BAD_REQUEST,
            rpc_error(id, -32600, "invalid request"),
        );
    }

    let method = request.method.trim().to_string();
    state.metrics.record_rpc(&method);
    log_dd_event(
        "INFO",
        9,
        "MCP JSON-RPC request",
        "cluster_mcp.rpc.request",
        json!({ "rpc.method": method }),
        Some(&trace),
    );

    if method == "notifications/initialized" {
        finish_span(
            state,
            trace,
            "mcp.rpc.notifications_initialized",
            true,
            json!({ "rpc.method": method }),
        );
        return empty_response(StatusCode::ACCEPTED);
    }

    let response = match method.as_str() {
        "initialize" => initialize_result(id),
        "ping" => json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
        "tools/list" => tools_list_result(id),
        "tools/call" => tools_call_result(&state, id, request.params.as_ref()).await,
        _ => {
            state
                .metrics
                .rpc_errors_total
                .fetch_add(1, Ordering::Relaxed);
            rpc_error(id, -32601, "method not found")
        }
    };
    let ok = response.get("error").is_none();
    finish_span(
        state,
        trace,
        "mcp.rpc.request",
        ok,
        json!({ "rpc.method": method }),
    );
    json_response(StatusCode::OK, response)
}

fn initialize_result(id: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {
                "tools": { "listChanged": false }
            },
            "serverInfo": {
                "name": SERVICE_NAME,
                "title": "DD Cluster MCP Rust Server",
                "version": SERVICE_VERSION,
                "description": "Rust MCP endpoint for the DD remote Kubernetes runtime"
            },
            "instructions": "Use tools/list to inspect read-only cluster runtime helpers. This server exposes Prometheus metrics at /metrics, dd.log.v1 stdout events for Loki, and explicit OTLP spans when configured."
        }
    })
}

fn tools_list_result(id: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "tools": [
                tool_def("cluster_status", "Cluster status", "Return service, gateway, and telemetry wiring for the DD remote Kubernetes runtime."),
                tool_def("service_directory", "Service directory", "List public and internal service paths plus live Kubernetes Service metadata visible to the read-only MCP service account."),
                tool_def("kubernetes_inventory", "Kubernetes inventory", "Read bounded metadata inventory for namespaces, nodes, workloads, pods, services, ingress, events, storage, autoscaling, and CRDs. Excludes Secrets, configmap data, pod logs, exec, and mutations."),
                tool_def("kubernetes_deployments", "Kubernetes deployments", "Read deployment metadata across namespaces from the in-cluster Kubernetes API using the MCP service account."),
                tool_def("human_access_policy", "Human access policy", "Explain the human-authenticated gateway, VPN, and bastion access model for sensitive operations. This tool never returns secrets or grants elevated access."),
                tool_def("telemetry_targets", "Telemetry targets", "List in-cluster observability endpoints, safe queries, and dashboard paths for this runtime."),
                tool_def("telemetry_summary", "Telemetry summary", "Read a bounded parallel summary from Prometheus, Loki, Grafana, Tempo, Jaeger, the OTel collector, and NATS metrics endpoints."),
                tool_def("observability_health", "Observability health", "Read live health from Prometheus, Loki, Grafana, Tempo, Jaeger, the OTel collector, and NATS through bounded in-cluster HTTP calls."),
                tool_def("prometheus_up", "Prometheus up query", "Run the safe Prometheus instant query `up` so agents can see which scrape targets are reachable."),
                tool_def("loki_labels", "Loki labels", "Read Loki label names to confirm container logs are flowing through promtail."),
                tool_def("grafana_inventory", "Grafana inventory", "Read Grafana datasource and dashboard inventory so agents can discover available observability views."),
                tool_def("nats_metrics", "NATS metrics", "Read NATS server /varz and the Prometheus exporter /metrics endpoint for messaging telemetry."),
                tool_def("trace_backends", "Trace backends", "Read Tempo readiness and Jaeger service discovery to confirm OTLP trace export/query wiring.")
            ]
        }
    })
}

fn tool_def(name: &str, title: &str, description: &str) -> Value {
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": { "type": "object", "properties": {} },
        "annotations": {
            "readOnlyHint": true,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false
        }
    })
}

async fn tools_call_result(state: &AppState, id: Value, params: Option<&Value>) -> Value {
    let tool = params
        .and_then(|params| params.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    state.metrics.record_tool(tool);
    let trace = new_trace_context();
    log_dd_event(
        "INFO",
        9,
        "MCP tool call",
        "cluster_mcp.tool.call",
        json!({ "mcp.tool": tool }),
        Some(&trace),
    );

    let result = match tool {
        "cluster_status" => tool_json_result(
            id,
            tool,
            "DD remote Kubernetes runtime MCP status.",
            cluster_status_json(state),
        ),
        "service_directory" => tool_json_result(
            id,
            tool,
            "Gateway, observability, and live Kubernetes service directory.",
            service_directory_json(state).await,
        ),
        "kubernetes_inventory" => tool_json_result(
            id,
            tool,
            "Bounded Kubernetes cluster inventory metadata visible to the read-only MCP service account.",
            kubernetes_inventory_json(state).await,
        ),
        "kubernetes_deployments" => tool_json_result(
            id,
            tool,
            "Kubernetes deployment metadata visible to the read-only MCP service account.",
            kubernetes_deployments_json(state).await,
        ),
        "human_access_policy" => tool_json_result(
            id,
            tool,
            "Human-authenticated access policy for the DD runtime gateway, MCP, VPN, and bastion.",
            human_access_policy_json(),
        ),
        "telemetry_targets" => tool_json_result(
            id,
            tool,
            "In-cluster observability endpoints and safe read-only queries.",
            telemetry_targets_json(state),
        ),
        "telemetry_summary" => tool_json_result(
            id,
            tool,
            "Bounded parallel telemetry summary from the in-cluster observability and NATS endpoints.",
            telemetry_summary_json(state).await,
        ),
        "observability_health" => tool_json_result(
            id,
            tool,
            "Live bounded health checks for Prometheus, Loki, Grafana, Tempo, Jaeger, the OTel collector, and NATS.",
            observability_health_json(state).await,
        ),
        "prometheus_up" => tool_json_result(
            id,
            tool,
            "Prometheus instant query `up` returned from the in-cluster Prometheus API.",
            prometheus_up_json(state).await,
        ),
        "loki_labels" => tool_json_result(
            id,
            tool,
            "Loki label names returned from the in-cluster Loki API.",
            loki_labels_json(state).await,
        ),
        "grafana_inventory" => tool_json_result(
            id,
            tool,
            "Grafana datasource and dashboard inventory returned from the in-cluster Grafana API.",
            grafana_inventory_json(state).await,
        ),
        "nats_metrics" => tool_json_result(
            id,
            tool,
            "NATS /varz and Prometheus exporter metrics returned from the in-cluster messaging service.",
            nats_metrics_json(state).await,
        ),
        "trace_backends" => tool_json_result(
            id,
            tool,
            "Tempo readiness and Jaeger service discovery returned from in-cluster trace backends.",
            trace_backends_json(state).await,
        ),
        _ => {
            state.metrics.rpc_errors_total.fetch_add(1, Ordering::Relaxed);
            state.metrics.tool_failures_total.fetch_add(1, Ordering::Relaxed);
            rpc_error(id, -32602, "unknown tool")
        }
    };

    finish_span(
        state.clone(),
        trace,
        "mcp.tool.call",
        result.get("error").is_none(),
        json!({ "mcp.tool": tool }),
    );
    result
}

fn tool_json_result(id: Value, tool: &str, text: &str, structured_content: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{ "type": "text", "text": text }],
            "structuredContent": structured_content,
            "_meta": { "tool": tool }
        }
    })
}

fn cluster_status_json(state: &AppState) -> Value {
    json!({
        "service": SERVICE_NAME,
        "namespace": SERVICE_NAMESPACE,
        "language": "rust",
        "runtime": "tokio-axum",
        "protocolVersion": PROTOCOL_VERSION,
        "gatewayPath": "/cluster-mcp",
        "mcpPath": "/cluster-mcp",
        "metricsPath": "/cluster-mcp/metrics",
        "internalMcpUrl": "http://dd-cluster-mcp-rs.default.svc.cluster.local:8091/mcp",
        "readOnly": true,
        "telemetry": {
            "prometheusMetrics": "/metrics",
            "structuredLogs": "dd.log.v1 stdout collected by promtail",
            "otlpTraces": state
                .config
                .otlp_endpoint
                .as_deref()
                .map(sanitize_url_for_output)
                .unwrap_or_else(|| "disabled".to_string())
        },
        "observability": {
            "grafana": "/telemetry/",
            "prometheus": "/prometheus/",
            "loki": sanitize_url_for_output(&state.config.loki_url),
            "tempo": sanitize_url_for_output(&state.config.tempo_url),
            "jaeger": sanitize_url_for_output(&state.config.jaeger_url),
            "otelCollectorMetrics": sanitize_url_for_output(&state.config.otel_collector_metrics_url),
            "natsMonitor": sanitize_url_for_output(&state.config.nats_monitor_url),
            "natsMetrics": sanitize_url_for_output(&state.config.nats_metrics_url)
        }
    })
}

async fn service_directory_json(state: &AppState) -> Value {
    let services = kubernetes_services_summary(state).await;
    let deployments = kubernetes_resource_entry(
        state,
        ResourceTarget {
            name: "deployments",
            scope: "all-namespaces",
            path: "/apis/apps/v1/deployments?limit=500",
        },
        state.config.kubernetes_inventory_body_limit_bytes,
        true,
    )
    .await;

    json!({
        "service": SERVICE_NAME,
        "mode": "read-only service directory",
        "public": [
            "/cluster-mcp",
            "/cluster-mcp/home",
            "/cluster-mcp/healthz",
            "/cluster-mcp/metrics",
            "/telemetry/",
            "/prometheus/",
            "/nats/",
            "/nats-metrics/metrics"
        ],
        "legacy": [
            "/mcp",
            "/mcp/home",
            "/mcp/healthz",
            "/mcp/metrics"
        ],
        "internal": [
            "dd-cluster-mcp-rs.default.svc.cluster.local:8091",
            "dd-gleam-mcp-server.default.svc.cluster.local:8090",
            "dd-prometheus.observability.svc.cluster.local:9090",
            "dd-loki.observability.svc.cluster.local:3100",
            "dd-grafana.observability.svc.cluster.local:3000",
            "dd-tempo.observability.svc.cluster.local:3200",
            "dd-jaeger.observability.svc.cluster.local:16686",
            "dd-otel-collector.observability.svc.cluster.local:4317",
            "dd-otel-collector.observability.svc.cluster.local:4318",
            "dd-otel-collector.observability.svc.cluster.local:8889",
            "dd-nats.messaging.svc.cluster.local:4222",
            "dd-nats.messaging.svc.cluster.local:8222",
            "dd-nats.messaging.svc.cluster.local:7777"
        ],
        "liveKubernetes": {
            "services": services,
            "deployments": deployments
        }
    })
}

async fn kubernetes_inventory_json(state: &AppState) -> Value {
    let targets = inventory_targets();
    let mut futures = FuturesUnordered::new();
    for (index, target) in targets.iter().copied().enumerate() {
        let state = state.clone();
        futures.push(async move {
            let entry = kubernetes_resource_entry(
                &state,
                target,
                state.config.kubernetes_inventory_body_limit_bytes,
                true,
            )
            .await;
            (index, entry)
        });
    }

    let mut resources = Vec::new();
    while let Some(entry) = futures.next().await {
        resources.push(entry);
    }
    resources.sort_by_key(|(index, _)| *index);
    let resources = resources
        .into_iter()
        .map(|(_, entry)| entry)
        .collect::<Vec<_>>();

    json!({
        "source": "kubernetes-api",
        "scope": "cluster inventory metadata",
        "readOnly": true,
        "metadataOnlyRequest": true,
        "resources": resources,
        "excluded": [
            "secrets",
            "configmaps data",
            "pods/exec",
            "pods/log",
            "mutation verbs"
        ]
    })
}

async fn kubernetes_deployments_json(state: &AppState) -> Value {
    kubernetes_resource_json(
        state,
        "deployments",
        "all-namespaces",
        "/apis/apps/v1/deployments?limit=500",
        state.config.kubernetes_body_limit_bytes,
        true,
    )
    .await
}

fn inventory_targets() -> Vec<ResourceTarget> {
    vec![
        ResourceTarget {
            name: "namespaces",
            scope: "cluster",
            path: "/api/v1/namespaces?limit=500",
        },
        ResourceTarget {
            name: "nodes",
            scope: "cluster",
            path: "/api/v1/nodes?limit=500",
        },
        ResourceTarget {
            name: "persistentvolumes",
            scope: "cluster",
            path: "/api/v1/persistentvolumes?limit=500",
        },
        ResourceTarget {
            name: "serviceaccounts",
            scope: "all-namespaces",
            path: "/api/v1/serviceaccounts?limit=500",
        },
        ResourceTarget {
            name: "pods",
            scope: "all-namespaces",
            path: "/api/v1/pods?limit=500",
        },
        ResourceTarget {
            name: "services",
            scope: "all-namespaces",
            path: "/api/v1/services?limit=500",
        },
        ResourceTarget {
            name: "endpoints",
            scope: "all-namespaces",
            path: "/api/v1/endpoints?limit=500",
        },
        ResourceTarget {
            name: "persistentvolumeclaims",
            scope: "all-namespaces",
            path: "/api/v1/persistentvolumeclaims?limit=500",
        },
        ResourceTarget {
            name: "events",
            scope: "all-namespaces",
            path: "/api/v1/events?limit=500",
        },
        ResourceTarget {
            name: "deployments",
            scope: "all-namespaces",
            path: "/apis/apps/v1/deployments?limit=500",
        },
        ResourceTarget {
            name: "daemonsets",
            scope: "all-namespaces",
            path: "/apis/apps/v1/daemonsets?limit=500",
        },
        ResourceTarget {
            name: "replicasets",
            scope: "all-namespaces",
            path: "/apis/apps/v1/replicasets?limit=500",
        },
        ResourceTarget {
            name: "statefulsets",
            scope: "all-namespaces",
            path: "/apis/apps/v1/statefulsets?limit=500",
        },
        ResourceTarget {
            name: "jobs",
            scope: "all-namespaces",
            path: "/apis/batch/v1/jobs?limit=500",
        },
        ResourceTarget {
            name: "cronjobs",
            scope: "all-namespaces",
            path: "/apis/batch/v1/cronjobs?limit=500",
        },
        ResourceTarget {
            name: "ingresses",
            scope: "all-namespaces",
            path: "/apis/networking.k8s.io/v1/ingresses?limit=500",
        },
        ResourceTarget {
            name: "networkpolicies",
            scope: "all-namespaces",
            path: "/apis/networking.k8s.io/v1/networkpolicies?limit=500",
        },
        ResourceTarget {
            name: "horizontalpodautoscalers",
            scope: "all-namespaces",
            path: "/apis/autoscaling/v2/horizontalpodautoscalers?limit=500",
        },
        ResourceTarget {
            name: "storageclasses",
            scope: "cluster",
            path: "/apis/storage.k8s.io/v1/storageclasses?limit=500",
        },
        ResourceTarget {
            name: "customresourcedefinitions",
            scope: "cluster",
            path: "/apis/apiextensions.k8s.io/v1/customresourcedefinitions?limit=500",
        },
    ]
}

async fn kubernetes_resource_entry(
    state: &AppState,
    target: ResourceTarget,
    limit: usize,
    metadata_only: bool,
) -> Value {
    json!({
        "name": target.name,
        "scope": target.scope,
        "path": target.path,
        "url": sanitize_url_for_output(&join_url(&state.config.kubernetes_api_url, target.path)),
        "response": kubernetes_get(state, target.path, limit, metadata_only).await
    })
}

async fn kubernetes_resource_json(
    state: &AppState,
    name: &str,
    scope: &str,
    path: &str,
    limit: usize,
    metadata_only: bool,
) -> Value {
    json!({
        "source": "kubernetes-api",
        "resource": name,
        "scope": scope,
        "url": sanitize_url_for_output(&join_url(&state.config.kubernetes_api_url, path)),
        "readOnly": true,
        "metadataOnlyRequest": metadata_only,
        "response": kubernetes_get(state, path, limit, metadata_only).await
    })
}

async fn kubernetes_get(state: &AppState, path: &str, limit: usize, metadata_only: bool) -> Value {
    state
        .metrics
        .k8s_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let started = Instant::now();
    let token = match tokio::fs::read_to_string(&state.config.kubernetes_token_path).await {
        Ok(token) => token.trim().to_string(),
        Err(error) => {
            return json!({
                "ok": false,
                "durationMs": elapsed_ms(started),
                "error": format!("failed to read service account token: {error}")
            });
        }
    };
    let url = join_url(&state.config.kubernetes_api_url, path);
    let mut request = state
        .k8s_http
        .get(url)
        .timeout(state.config.kubernetes_timeout)
        .bearer_auth(token);
    if metadata_only {
        request = request.header(
            header::ACCEPT,
            "application/json;as=PartialObjectMetadataList;g=meta.k8s.io;v=v1",
        );
    } else {
        request = request.header(header::ACCEPT, "application/json");
    }

    match request.send().await {
        Ok(response) => {
            let status = response.status();
            let reason = status.canonical_reason().unwrap_or("").to_string();
            match response.bytes().await {
                Ok(bytes) => {
                    let body = String::from_utf8_lossy(&bytes).to_string();
                    let (sample, truncated) = response_sample(&body, limit);
                    let (item_count, items) = summarize_kubernetes_items(
                        &body,
                        state.config.kubernetes_items_limit,
                        metadata_only,
                    );
                    json!({
                        "ok": status.is_success(),
                        "status": status.as_u16(),
                        "reason": reason,
                        "durationMs": elapsed_ms(started),
                        "truncated": truncated,
                        "itemCount": item_count,
                        "items": items,
                        "sample": sample
                    })
                }
                Err(error) => json!({
                    "ok": false,
                    "status": status.as_u16(),
                    "reason": reason,
                    "durationMs": elapsed_ms(started),
                    "error": sanitize_text(&error.to_string())
                }),
            }
        }
        Err(error) => json!({
            "ok": false,
            "durationMs": elapsed_ms(started),
            "error": sanitize_text(&error.to_string())
        }),
    }
}

fn summarize_kubernetes_items(
    body: &str,
    limit: usize,
    metadata_only: bool,
) -> (Option<usize>, Vec<Value>) {
    let Ok(parsed) = serde_json::from_str::<Value>(body) else {
        return (None, Vec::new());
    };
    let Some(items) = parsed.get("items").and_then(Value::as_array) else {
        return (None, Vec::new());
    };
    let summary = items
        .iter()
        .take(limit)
        .map(|item| summarize_kubernetes_item(item, metadata_only))
        .collect::<Vec<_>>();
    (Some(items.len()), summary)
}

fn summarize_kubernetes_item(item: &Value, metadata_only: bool) -> Value {
    let metadata = item.get("metadata").unwrap_or(&Value::Null);
    let mut output = Map::new();
    output.insert(
        "apiVersion".to_string(),
        item.get("apiVersion").cloned().unwrap_or(Value::Null),
    );
    output.insert(
        "kind".to_string(),
        item.get("kind").cloned().unwrap_or(Value::Null),
    );
    output.insert(
        "name".to_string(),
        metadata.get("name").cloned().unwrap_or(Value::Null),
    );
    output.insert(
        "namespace".to_string(),
        metadata.get("namespace").cloned().unwrap_or(Value::Null),
    );
    output.insert(
        "creationTimestamp".to_string(),
        metadata
            .get("creationTimestamp")
            .cloned()
            .unwrap_or(Value::Null),
    );
    output.insert(
        "labels".to_string(),
        metadata
            .get("labels")
            .map(redacted_json_clone)
            .unwrap_or_else(|| json!({})),
    );
    if !metadata_only {
        if let Some(spec) = item.get("spec") {
            output.insert("specSummary".to_string(), summarize_service_spec(spec));
        }
    }
    Value::Object(output)
}

async fn kubernetes_services_summary(state: &AppState) -> Value {
    let result = kubernetes_get(
        state,
        "/api/v1/services?limit=500",
        state.config.kubernetes_body_limit_bytes,
        false,
    )
    .await;
    json!({
        "source": "kubernetes-api",
        "resource": "services",
        "scope": "all-namespaces",
        "readOnly": true,
        "response": result
    })
}

fn summarize_service_spec(spec: &Value) -> Value {
    json!({
        "type": spec.get("type").cloned().unwrap_or(Value::Null),
        "clusterIP": spec.get("clusterIP").cloned().unwrap_or(Value::Null),
        "selector": spec.get("selector").cloned().unwrap_or_else(|| json!({})),
        "ports": spec
            .get("ports")
            .and_then(Value::as_array)
            .map(|ports| {
                ports
                    .iter()
                    .map(|port| {
                        json!({
                            "name": port.get("name").cloned().unwrap_or(Value::Null),
                            "protocol": port.get("protocol").cloned().unwrap_or(Value::Null),
                            "port": port.get("port").cloned().unwrap_or(Value::Null),
                            "targetPort": port.get("targetPort").cloned().unwrap_or(Value::Null)
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    })
}

fn human_access_policy_json() -> Value {
    json!({
        "source": "mcp-policy",
        "readOnlyByDefault": true,
        "humanAuthRequiredForPublicGateway": true,
        "humanAuthPath": "/auth?return=/cluster-mcp/home",
        "acceptedGatewayProofs": [
            "dd_auth HttpOnly cookie from dd-remote-auth",
            "Auth header with the gateway secret for non-browser callers"
        ],
        "recommendedHumanProof": "operator passphrase plus optional TOTP on dd-remote-auth",
        "elevatedMcpToolsEnabled": false,
        "sensitiveKubernetesAccess": [
            "Do not expose Kubernetes Secrets through MCP",
            "Do not expose ConfigMap data through MCP",
            "Do not expose pod logs or pod exec through MCP",
            "Use VPN plus dd-bastion, SSM, or SSH for human shell access"
        ],
        "auditExpectation": "Add a separate short-lived grant service before enabling any write, secret, log, or exec-capable MCP tool."
    })
}

fn telemetry_targets_json(state: &AppState) -> Value {
    json!({
        "service": SERVICE_NAME,
        "mode": "read-only",
        "timeoutMs": state.config.observability_timeout.as_millis() as u64,
        "bodyLimitBytes": state.config.observability_body_limit_bytes,
        "targets": [
            target_obj("prometheus", &state.config.prometheus_url, "metrics/query store"),
            target_obj("loki", &state.config.loki_url, "log store"),
            target_obj("grafana", &state.config.grafana_url, "dashboard UI/API"),
            target_obj("tempo", &state.config.tempo_url, "trace store"),
            target_obj("jaeger", &state.config.jaeger_url, "trace query UI/API"),
            target_obj("otelCollectorPrometheus", &join_url(&state.config.otel_collector_metrics_url, "/metrics"), "collector exported metrics"),
            target_obj("natsMonitor", &state.config.nats_monitor_url, "NATS server monitoring API"),
            target_obj("natsMetrics", &join_url(&state.config.nats_metrics_url, "/metrics"), "NATS Prometheus exporter")
        ],
        "safeQueries": [
            { "name": "prometheus_up", "query": "up" },
            { "name": "loki_labels", "path": "/loki/api/v1/labels" },
            { "name": "grafana_datasources", "path": "/api/datasources" },
            { "name": "grafana_dashboards", "path": "/api/search?type=dash-db" },
            { "name": "jaeger_services", "path": "/api/services" },
            { "name": "nats_varz", "path": "/varz" },
            { "name": "nats_exporter_metrics", "path": "/metrics" }
        ]
    })
}

fn target_obj(name: &str, url: &str, role: &str) -> Value {
    json!({ "name": name, "url": sanitize_url_for_output(url), "role": role })
}

async fn observability_health_json(state: &AppState) -> Value {
    let checks = parallel_http_checks(
        state,
        vec![
            (
                "prometheus",
                join_url(&state.config.prometheus_url, "/-/healthy"),
                2048,
            ),
            ("loki", join_url(&state.config.loki_url, "/ready"), 2048),
            (
                "grafana",
                join_url(&state.config.grafana_url, "/api/health"),
                4096,
            ),
            ("tempo", join_url(&state.config.tempo_url, "/ready"), 2048),
            (
                "jaeger",
                join_url(&state.config.jaeger_url, "/api/services"),
                4096,
            ),
            (
                "otelCollectorMetrics",
                join_url(&state.config.otel_collector_metrics_url, "/metrics"),
                4096,
            ),
            (
                "natsMonitor",
                join_url(&state.config.nats_monitor_url, "/healthz"),
                2048,
            ),
            (
                "natsMetrics",
                join_url(&state.config.nats_metrics_url, "/metrics"),
                4096,
            ),
        ],
    )
    .await;
    json!({
        "service": SERVICE_NAME,
        "mode": "read-only observability health",
        "checks": checks
    })
}

async fn telemetry_summary_json(state: &AppState) -> Value {
    let sources = parallel_http_checks(
        state,
        vec![
            (
                "prometheusHealthy",
                join_url(&state.config.prometheus_url, "/-/healthy"),
                2048,
            ),
            (
                "prometheusTargets",
                join_url(&state.config.prometheus_url, "/api/v1/targets?state=active"),
                state.config.observability_body_limit_bytes,
            ),
            (
                "lokiReady",
                join_url(&state.config.loki_url, "/ready"),
                2048,
            ),
            (
                "lokiLabels",
                join_url(&state.config.loki_url, "/loki/api/v1/labels"),
                state.config.observability_body_limit_bytes,
            ),
            (
                "grafanaHealth",
                join_url(&state.config.grafana_url, "/api/health"),
                4096,
            ),
            (
                "grafanaDatasources",
                join_url(&state.config.grafana_url, "/api/datasources"),
                state.config.observability_body_limit_bytes,
            ),
            (
                "grafanaDashboards",
                join_url(&state.config.grafana_url, "/api/search?type=dash-db"),
                state.config.observability_body_limit_bytes,
            ),
            (
                "tempoReady",
                join_url(&state.config.tempo_url, "/ready"),
                2048,
            ),
            (
                "jaegerServices",
                join_url(&state.config.jaeger_url, "/api/services"),
                state.config.observability_body_limit_bytes,
            ),
            (
                "otelCollectorMetrics",
                join_url(&state.config.otel_collector_metrics_url, "/metrics"),
                state.config.observability_body_limit_bytes,
            ),
            (
                "natsVarz",
                join_url(&state.config.nats_monitor_url, "/varz"),
                state.config.observability_body_limit_bytes,
            ),
            (
                "natsExporterMetrics",
                join_url(&state.config.nats_metrics_url, "/metrics"),
                state.config.observability_body_limit_bytes,
            ),
        ],
    )
    .await;
    json!({
        "service": SERVICE_NAME,
        "mode": "bounded read-only telemetry summary",
        "sources": sources
    })
}

async fn prometheus_up_json(state: &AppState) -> Value {
    let query = "up";
    let url = join_url(&state.config.prometheus_url, "/api/v1/query?query=up");
    json!({
        "service": SERVICE_NAME,
        "source": "prometheus",
        "query": query,
        "result": http_result(state, &url, state.config.observability_body_limit_bytes).await
    })
}

async fn loki_labels_json(state: &AppState) -> Value {
    let path = "/loki/api/v1/labels";
    let url = join_url(&state.config.loki_url, path);
    json!({
        "service": SERVICE_NAME,
        "source": "loki",
        "path": path,
        "result": http_result(state, &url, state.config.observability_body_limit_bytes).await
    })
}

async fn grafana_inventory_json(state: &AppState) -> Value {
    json!({
        "service": SERVICE_NAME,
        "source": "grafana",
        "datasources": http_result(
            state,
            &join_url(&state.config.grafana_url, "/api/datasources"),
            state.config.observability_body_limit_bytes,
        )
        .await,
        "dashboards": http_result(
            state,
            &join_url(&state.config.grafana_url, "/api/search?type=dash-db"),
            state.config.observability_body_limit_bytes,
        )
        .await
    })
}

async fn nats_metrics_json(state: &AppState) -> Value {
    json!({
        "service": SERVICE_NAME,
        "source": "nats",
        "monitor": http_result(
            state,
            &join_url(&state.config.nats_monitor_url, "/varz"),
            state.config.observability_body_limit_bytes,
        )
        .await,
        "metrics": http_result(
            state,
            &join_url(&state.config.nats_metrics_url, "/metrics"),
            state.config.observability_body_limit_bytes,
        )
        .await
    })
}

async fn trace_backends_json(state: &AppState) -> Value {
    let checks = parallel_http_checks(
        state,
        vec![
            (
                "tempoReady",
                join_url(&state.config.tempo_url, "/ready"),
                4096,
            ),
            (
                "jaegerServices",
                join_url(&state.config.jaeger_url, "/api/services"),
                state.config.observability_body_limit_bytes,
            ),
        ],
    )
    .await;
    json!({
        "service": SERVICE_NAME,
        "mode": "trace backend read-only summary",
        "checks": checks
    })
}

async fn parallel_http_checks(
    state: &AppState,
    checks: Vec<(&'static str, String, usize)>,
) -> Vec<Value> {
    let mut futures = FuturesUnordered::new();
    for (index, (name, url, limit)) in checks.into_iter().enumerate() {
        let state = state.clone();
        futures.push(async move {
            let result = http_result(&state, &url, limit).await;
            (
                index,
                json!({ "name": name, "url": sanitize_url_for_output(&url), "result": result }),
            )
        });
    }

    let mut output = Vec::new();
    while let Some(item) = futures.next().await {
        output.push(item);
    }
    output.sort_by_key(|(index, _)| *index);
    output.into_iter().map(|(_, value)| value).collect()
}

async fn http_result(state: &AppState, url: &str, limit: usize) -> Value {
    state
        .metrics
        .observability_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let started = Instant::now();
    match state
        .http
        .get(url)
        .timeout(state.config.observability_timeout)
        .header(header::ACCEPT, "application/json,text/plain,*/*")
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status();
            let reason = status.canonical_reason().unwrap_or("").to_string();
            match response.bytes().await {
                Ok(bytes) => {
                    let body = String::from_utf8_lossy(&bytes).to_string();
                    let (sample, truncated) = response_sample(&body, limit);
                    json!({
                        "ok": status.is_success(),
                        "status": status.as_u16(),
                        "reason": reason,
                        "durationMs": elapsed_ms(started),
                        "truncated": truncated,
                        "sample": sample
                    })
                }
                Err(error) => json!({
                    "ok": false,
                    "status": status.as_u16(),
                    "reason": reason,
                    "durationMs": elapsed_ms(started),
                    "error": sanitize_text(&error.to_string())
                }),
            }
        }
        Err(error) => json!({
            "ok": false,
            "durationMs": elapsed_ms(started),
            "error": sanitize_text(&error.to_string())
        }),
    }
}

fn join_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    if path.is_empty() {
        return base.to_string();
    }
    if path.starts_with('/') {
        format!("{base}{path}")
    } else {
        format!("{base}/{path}")
    }
}

fn clip_string(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_string();
    }
    if limit <= 32 {
        return value.chars().take(limit).collect();
    }
    let mut clipped = String::new();
    let mut used = 0usize;
    for ch in value.chars() {
        let next = used + ch.len_utf8();
        if next > limit {
            break;
        }
        clipped.push(ch);
        used = next;
    }
    clipped.push_str("\n... clipped ...");
    clipped
}

fn response_sample(body: &str, limit: usize) -> (String, bool) {
    let sanitized = sanitize_response_body(body);
    let truncated = sanitized.len() > limit;
    (clip_string(&sanitized, limit), truncated)
}

fn sanitize_response_body(body: &str) -> String {
    match serde_json::from_str::<Value>(body) {
        Ok(mut value) => {
            redact_json_value(&mut value);
            value.to_string()
        }
        Err(_) => sanitize_text(body),
    }
}

fn redacted_json_clone(value: &Value) -> Value {
    let mut value = value.clone();
    redact_json_value(&mut value);
    value
}

fn redact_json_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, value) in map.iter_mut() {
                if is_secret_like_key(key) {
                    *value = json!(REDACTED);
                } else {
                    redact_json_value(value);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                redact_json_value(item);
            }
        }
        _ => {}
    }
}

fn is_secret_like_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    let normalized = key.replace(['-', '_', '.'], "");
    key.contains("authorization")
        || key.contains("cookie")
        || key.contains("credential")
        || key.contains("password")
        || key.contains("secret")
        || key.contains("session")
        || key.contains("token")
        || normalized.contains("apikey")
        || normalized.contains("accesskey")
        || normalized.contains("privatekey")
        || normalized.contains("clientsecret")
}

fn sanitize_text(value: &str) -> String {
    value
        .lines()
        .map(redact_sensitive_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn redact_sensitive_line(line: &str) -> String {
    if !line
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-')
        .any(is_secret_like_key)
    {
        return line.to_string();
    }

    if let Some(index) = line.find('=').or_else(|| line.find(':')) {
        format!("{}{}", &line[..=index], REDACTED)
    } else {
        REDACTED.to_string()
    }
}

fn sanitize_url_for_output(raw: &str) -> String {
    match reqwest::Url::parse(raw) {
        Ok(mut url) => {
            if !url.username().is_empty() {
                let _ = url.set_username("redacted");
            }
            if url.password().is_some() {
                let _ = url.set_password(Some("redacted"));
            }
            if url.query().is_some() {
                url.set_query(Some("redacted=1"));
            }
            url.set_fragment(None);
            url.to_string()
        }
        Err(_) => sanitize_text(raw),
    }
}

async fn api_docs_html() -> Html<&'static str> {
    Html(include_str!("../generated/api-docs.html"))
}

async fn api_docs_json() -> impl IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}

#[tokio::main]
async fn main() {
    let _otel = dd_telemetry::init("dd-cluster-mcp-rs");

    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    let config = Arc::new(config_from_env());
    let state = AppState {
        http: build_http_client(&config),
        k8s_http: build_k8s_client(&config),
        metrics: Arc::new(Metrics::default()),
        config,
    };

    let app = Router::new()
        .route("/", get(root).post(rpc))
        .route("/home", get(home))
        .route("/healthz", get(healthz))
        .route("/readyz", get(healthz))
        .route("/metrics", get(metrics))
        .route("/observability", get(observability))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/mcp", get(mcp_get).post(rpc))
        .with_state(state.clone())
        // Merge the runtime-config /internal/* routes BEFORE applying the body
        // limit so the cap covers them too (layer() only wraps routes added so
        // far; a merge after it would leave /internal/* on axum's 2 MiB default).
        .merge(dd_runtime_config_client::router())
        .layer(DefaultBodyLimit::max(MAX_RPC_BODY_BYTES));

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let address: SocketAddr = format!("{}:{}", state.config.host, state.config.port)
        .parse()
        .expect("failed to parse bind address");
    log_dd_event(
        "INFO",
        9,
        "dd-cluster-mcp-rs listening",
        "cluster_mcp.server.listening",
        json!({ "address": address.to_string(), "port": state.config.port }),
        None,
    );

    let listener = tokio::net::TcpListener::bind(address)
        .await
        .expect("failed to bind tcp listener");
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("axum server crashed");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut sigterm) = signal(SignalKind::terminate()) {
            let _ = sigterm.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_url_handles_slashes() {
        assert_eq!(
            join_url("http://example.test/", "/ready"),
            "http://example.test/ready"
        );
        assert_eq!(
            join_url("http://example.test", "ready"),
            "http://example.test/ready"
        );
        assert_eq!(join_url("http://example.test/", ""), "http://example.test");
    }

    #[test]
    fn clip_string_preserves_utf8_boundaries() {
        let clipped = clip_string(
            "alpha beta gamma delta epsilon zeta eta theta iota kappa",
            40,
        );
        assert!(clipped.starts_with("alpha beta gamma"));
        assert!(clipped.ends_with("... clipped ..."));
        let prefix = "a".repeat(32);
        let unicode = clip_string(&format!("{prefix}\u{00e9}omega"), 33);
        assert_eq!(unicode.trim_end_matches("\n... clipped ..."), prefix);
    }

    #[test]
    fn response_sample_redacts_secret_like_json_fields() {
        let body = json!({
            "ok": true,
            "token": "abc123",
            "nested": {
                "client_secret": "shh",
                "safe": "visible"
            },
            "items": [{ "authorization": "Bearer secret-token" }]
        })
        .to_string();
        let (sample, truncated) = response_sample(&body, 10_000);
        assert!(!truncated);
        assert!(sample.contains(REDACTED));
        assert!(sample.contains("visible"));
        assert!(!sample.contains("abc123"));
        assert!(!sample.contains("secret-token"));
    }

    #[test]
    fn response_sample_redacts_secret_like_text_lines() {
        let (sample, _) = response_sample("ready\napi_key=super-secret\nconnections 3", 10_000);
        assert!(sample.contains("ready"));
        assert!(sample.contains(&format!("api_key={REDACTED}")));
        assert!(!sample.contains("super-secret"));
    }

    #[test]
    fn allowed_mcp_base_url_stays_inside_cluster_or_loopback() {
        assert!(allowed_mcp_base_url(
            "http://dd-prometheus.observability.svc.cluster.local:9090"
        ));
        assert!(allowed_mcp_base_url("http://127.0.0.1:19090"));
        assert!(!allowed_mcp_base_url("https://example.com"));
        assert!(!allowed_mcp_base_url(
            "http://dd-prometheus.observability.svc.cluster.local:9090?token=abc"
        ));
        assert!(!allowed_mcp_base_url("file:///var/run/secrets/token"));
    }

    #[test]
    fn sanitize_url_for_output_removes_url_credentials_and_query() {
        assert_eq!(
            sanitize_url_for_output("https://user:pass@example.test/path?token=abc#frag"),
            "https://redacted:redacted@example.test/path?redacted=1"
        );
    }

    #[test]
    fn json_rpc_ids_are_scalar_only() {
        assert_eq!(sanitize_rpc_id(Some(&json!("abc"))), json!("abc"));
        assert_eq!(sanitize_rpc_id(Some(&json!(42))), json!(42));
        assert_eq!(sanitize_rpc_id(Some(&json!(null))), json!(null));
        assert_eq!(
            sanitize_rpc_id(Some(&json!({ "not": "allowed" }))),
            json!(null)
        );
        assert_eq!(sanitize_rpc_id(None), json!(null));
    }

    #[test]
    fn service_summary_omits_annotations() {
        let item = json!({
            "apiVersion": "v1",
            "kind": "Service",
            "metadata": {
                "name": "dd-example",
                "namespace": "default",
                "annotations": { "kubectl.kubernetes.io/last-applied-configuration": "large" },
                "labels": { "app": "dd-example", "token": "abc123" }
            },
            "spec": {
                "type": "ClusterIP",
                "clusterIP": "10.0.0.10",
                "selector": { "app": "dd-example" },
                "ports": [{ "name": "http", "port": 8080, "targetPort": 8080, "protocol": "TCP" }]
            }
        });
        let summary = summarize_kubernetes_item(&item, false);
        assert!(summary.get("annotations").is_none());
        assert_eq!(summary["specSummary"]["ports"][0]["port"], json!(8080));
        assert_eq!(summary["labels"]["token"], json!(REDACTED));
    }

    #[test]
    fn tool_catalog_keeps_expected_names() {
        let listed = TOOL_NAMES.iter().copied().collect::<Vec<_>>();
        assert!(listed.contains(&"kubernetes_inventory"));
        assert!(listed.contains(&"telemetry_summary"));
        assert!(listed.contains(&"trace_backends"));
    }

    #[test]
    fn metric_labels_bucket_unknown_to_other() {
        // Known names pass through; arbitrary caller-supplied names collapse to
        // "other" so the metric maps can't be grown without bound.
        assert_eq!(bounded_method_label("tools/call"), "tools/call");
        assert_eq!(bounded_method_label("initialize"), "initialize");
        assert_eq!(bounded_method_label("evil/../../etc/passwd"), "other");
        assert_eq!(bounded_method_label(""), "other");
        assert_eq!(bounded_tool_label("kubernetes_inventory"), "kubernetes_inventory");
        assert_eq!(bounded_tool_label("attacker-supplied-name"), "other");
    }

    #[test]
    fn header_secret_gate_accepts_only_matching_credentials() {
        let mut headers = HeaderMap::new();
        // No secret configured => fail closed even with a header present.
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer s3cret"));
        assert!(!header_secret_ok(None, &headers));
        assert!(!header_secret_ok(Some(""), &headers));
        // Correct bearer.
        assert!(header_secret_ok(Some("s3cret"), &headers));
        // Wrong bearer.
        assert!(!header_secret_ok(Some("nope"), &headers));
        // X-Server-Auth path.
        let mut xheaders = HeaderMap::new();
        xheaders.insert("x-server-auth", HeaderValue::from_static("s3cret"));
        assert!(header_secret_ok(Some("s3cret"), &xheaders));
        // Missing entirely.
        assert!(!header_secret_ok(Some("s3cret"), &HeaderMap::new()));
    }
}
