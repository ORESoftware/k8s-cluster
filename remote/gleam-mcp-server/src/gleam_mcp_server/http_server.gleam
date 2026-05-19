import gleam/bit_array
import gleam/bytes_tree
import gleam/erlang/process
import gleam/http.{Get, Post}
import gleam/http/request
import gleam/http/response
import gleam/int
import gleam/io
import gleam/result
import gleam/string
import gleam_mcp_server/k8s
import gleam_mcp_server/metrics
import gleam_mcp_server/observability
import mist

@external(erlang, "gleam_mcp_runtime_env", "getenv")
fn env_get(name: String) -> String

@external(erlang, "gleam_mcp_json", "request_id")
fn json_request_id(body: String) -> String

const default_host = "0.0.0.0"

const default_port = 8090

const protocol_version = "2025-11-25"

pub fn supervised(metrics_name: process.Name(metrics.Message)) {
  mist.new(fn(req) { route(req, metrics_name) })
  |> mist.bind(bind_host())
  |> mist.port(bind_port())
  |> mist.supervised
}

pub fn bind_host() -> String {
  case env_get("HOST") {
    "" -> default_host
    value -> value
  }
}

pub fn bind_port() -> Int {
  case int.parse(env_get("PORT")) {
    Ok(value) -> {
      case value > 0 && value <= 65_535 {
        True -> value
        False -> default_port
      }
    }
    Error(_) -> default_port
  }
}

fn route(
  req: request.Request(mist.Connection),
  metrics_name: process.Name(metrics.Message),
) -> response.Response(mist.ResponseData) {
  record_http(metrics_name)

  case req.method, request.path_segments(req) {
    Get, [] -> redirect("/home")
    Get, ["home"] -> home_page()
    Get, ["healthz"] -> healthz()
    Get, ["metrics"] -> metrics_response(metrics_name)
    Get, ["observability"] -> observability_response()
    Get, ["mcp"] -> mcp_get(req)
    Post, ["mcp"] -> rpc(req, metrics_name)
    Post, [] -> rpc(req, metrics_name)
    _, _ -> not_found()
  }
}

fn rpc(
  req: request.Request(mist.Connection),
  metrics_name: process.Name(metrics.Message),
) -> response.Response(mist.ResponseData) {
  req
  |> mist.read_body(max_body_limit: 1_000_000)
  |> result.map(fn(req_with_body) {
    let body =
      req_with_body.body
      |> bit_array.to_string
      |> result.unwrap("")
    let method = method_from_body(body)
    record_rpc(metrics_name, method)
    io.println("dd-gleam-mcp-server rpc method=" <> method)

    case method {
      "notifications/initialized" -> empty_response(202)
      _ -> json_response(200, rpc_payload(method, body, json_request_id(body)))
    }
  })
  |> result.unwrap(json_response(400, json_rpc_error("parse error", -32_700)))
}

fn method_from_body(body: String) -> String {
  case string.contains(body, "\"tools/call\"") {
    True -> "tools/call"
    False ->
      case string.contains(body, "\"tools/list\"") {
        True -> "tools/list"
        False ->
          case string.contains(body, "\"initialize\"") {
            True -> "initialize"
            False ->
              case string.contains(body, "\"ping\"") {
                True -> "ping"
                False ->
                  case string.contains(body, "\"notifications/initialized\"") {
                    True -> "notifications/initialized"
                    False -> "unknown"
                  }
              }
          }
      }
  }
}

fn rpc_payload(method: String, body: String, request_id: String) -> String {
  case method {
    "initialize" -> initialize_result(request_id)
    "tools/list" -> tools_list_result(request_id)
    "tools/call" -> tools_call_result(tool_from_body(body), request_id)
    "ping" -> "{\"jsonrpc\":\"2.0\",\"id\":" <> request_id <> ",\"result\":{}}"
    _ -> json_rpc_error_with_id("method not found", -32_601, request_id)
  }
}

fn tool_from_body(body: String) -> String {
  case string.contains(body, "\"kubernetes_inventory\"") {
    True -> "kubernetes_inventory"
    False ->
      case string.contains(body, "\"kubernetes_deployments\"") {
        True -> "kubernetes_deployments"
        False ->
          case string.contains(body, "\"human_access_policy\"") {
            True -> "human_access_policy"
            False ->
              case string.contains(body, "\"telemetry_summary\"") {
                True -> "telemetry_summary"
                False ->
                  case string.contains(body, "\"observability_health\"") {
                    True -> "observability_health"
                    False ->
                      case string.contains(body, "\"prometheus_up\"") {
                        True -> "prometheus_up"
                        False ->
                          case string.contains(body, "\"loki_labels\"") {
                            True -> "loki_labels"
                            False ->
                              case
                                string.contains(body, "\"grafana_inventory\"")
                              {
                                True -> "grafana_inventory"
                                False ->
                                  case
                                    string.contains(body, "\"nats_metrics\"")
                                  {
                                    True -> "nats_metrics"
                                    False ->
                                      case
                                        string.contains(
                                          body,
                                          "\"trace_backends\"",
                                        )
                                      {
                                        True -> "trace_backends"
                                        False ->
                                          case
                                            string.contains(
                                              body,
                                              "\"telemetry_targets\"",
                                            )
                                          {
                                            True -> "telemetry_targets"
                                            False ->
                                              case
                                                string.contains(
                                                  body,
                                                  "\"service_directory\"",
                                                )
                                              {
                                                True -> "service_directory"
                                                False ->
                                                  case
                                                    string.contains(
                                                      body,
                                                      "\"cluster_status\"",
                                                    )
                                                  {
                                                    True -> "cluster_status"
                                                    False -> "unknown"
                                                  }
                                              }
                                          }
                                      }
                                  }
                              }
                          }
                      }
                  }
              }
          }
      }
  }
}

fn initialize_result(request_id: String) -> String {
  "{\"jsonrpc\":\"2.0\",\"id\":"
  <> request_id
  <> ",\"result\":{\"protocolVersion\":\""
  <> protocol_version
  <> "\",\"capabilities\":{\"tools\":{\"listChanged\":false}},\"serverInfo\":{\"name\":\"dd-gleam-mcp-server\",\"title\":\"DD Gleam MCP Server\",\"version\":\"0.1.0\",\"description\":\"Gleam MCP endpoint for the DD remote Kubernetes runtime\"},\"instructions\":\"Use tools/list to inspect read-only cluster runtime helpers. The service exports Prometheus metrics at /metrics and writes structured-ish request logs to stdout for Loki.\"}}"
}

fn tools_list_result(request_id: String) -> String {
  "{\"jsonrpc\":\"2.0\",\"id\":"
  <> request_id
  <> ",\"result\":{\"tools\":["
  <> "{\"name\":\"cluster_status\",\"title\":\"Cluster status\",\"description\":\"Return static service discovery details for the DD remote Kubernetes runtime.\",\"inputSchema\":{\"type\":\"object\",\"properties\":{}},\"annotations\":{\"readOnlyHint\":true,\"destructiveHint\":false,\"idempotentHint\":true,\"openWorldHint\":false}},"
  <> "{\"name\":\"service_directory\",\"title\":\"Service directory\",\"description\":\"List public and internal service paths exposed by the runtime gateway.\",\"inputSchema\":{\"type\":\"object\",\"properties\":{}},\"annotations\":{\"readOnlyHint\":true,\"destructiveHint\":false,\"idempotentHint\":true,\"openWorldHint\":false}},"
  <> "{\"name\":\"kubernetes_inventory\",\"title\":\"Kubernetes inventory\",\"description\":\"Read bounded metadata inventory for namespaces, nodes, workloads, pods, services, ingress, events, storage, autoscaling, and CRDs. Excludes Secrets, configmap data, pod logs, exec, and mutations.\",\"inputSchema\":{\"type\":\"object\",\"properties\":{}},\"annotations\":{\"readOnlyHint\":true,\"destructiveHint\":false,\"idempotentHint\":true,\"openWorldHint\":false}},"
  <> "{\"name\":\"kubernetes_deployments\",\"title\":\"Kubernetes deployments\",\"description\":\"Read all Kubernetes deployments across namespaces from the in-cluster Kubernetes API using the MCP service account.\",\"inputSchema\":{\"type\":\"object\",\"properties\":{}},\"annotations\":{\"readOnlyHint\":true,\"destructiveHint\":false,\"idempotentHint\":true,\"openWorldHint\":false}},"
  <> "{\"name\":\"human_access_policy\",\"title\":\"Human access policy\",\"description\":\"Explain the human-authenticated gateway, VPN, and bastion access model for sensitive operations. This tool never returns secrets or grants elevated access.\",\"inputSchema\":{\"type\":\"object\",\"properties\":{}},\"annotations\":{\"readOnlyHint\":true,\"destructiveHint\":false,\"idempotentHint\":true,\"openWorldHint\":false}},"
  <> "{\"name\":\"telemetry_targets\",\"title\":\"Telemetry targets\",\"description\":\"List in-cluster observability endpoints, safe queries, and dashboard paths for this runtime.\",\"inputSchema\":{\"type\":\"object\",\"properties\":{}},\"annotations\":{\"readOnlyHint\":true,\"destructiveHint\":false,\"idempotentHint\":true,\"openWorldHint\":false}},"
  <> "{\"name\":\"telemetry_summary\",\"title\":\"Telemetry summary\",\"description\":\"Read a bounded parallel summary from Prometheus, Loki, Grafana, Tempo, Jaeger, the OTel collector, and NATS metrics endpoints.\",\"inputSchema\":{\"type\":\"object\",\"properties\":{}},\"annotations\":{\"readOnlyHint\":true,\"destructiveHint\":false,\"idempotentHint\":true,\"openWorldHint\":false}},"
  <> "{\"name\":\"observability_health\",\"title\":\"Observability health\",\"description\":\"Read live health from Prometheus, Loki, Grafana, Tempo, Jaeger, and the OTel collector through bounded in-cluster HTTP calls.\",\"inputSchema\":{\"type\":\"object\",\"properties\":{}},\"annotations\":{\"readOnlyHint\":true,\"destructiveHint\":false,\"idempotentHint\":true,\"openWorldHint\":false}},"
  <> "{\"name\":\"prometheus_up\",\"title\":\"Prometheus up query\",\"description\":\"Run the safe Prometheus instant query `up` so agents can see which scrape targets are reachable.\",\"inputSchema\":{\"type\":\"object\",\"properties\":{}},\"annotations\":{\"readOnlyHint\":true,\"destructiveHint\":false,\"idempotentHint\":true,\"openWorldHint\":false}},"
  <> "{\"name\":\"loki_labels\",\"title\":\"Loki labels\",\"description\":\"Read Loki label names to confirm container logs are flowing through promtail.\",\"inputSchema\":{\"type\":\"object\",\"properties\":{}},\"annotations\":{\"readOnlyHint\":true,\"destructiveHint\":false,\"idempotentHint\":true,\"openWorldHint\":false}},"
  <> "{\"name\":\"grafana_inventory\",\"title\":\"Grafana inventory\",\"description\":\"Read Grafana datasource and dashboard inventory so agents can discover available observability views.\",\"inputSchema\":{\"type\":\"object\",\"properties\":{}},\"annotations\":{\"readOnlyHint\":true,\"destructiveHint\":false,\"idempotentHint\":true,\"openWorldHint\":false}},"
  <> "{\"name\":\"nats_metrics\",\"title\":\"NATS metrics\",\"description\":\"Read NATS server /varz and the Prometheus exporter /metrics endpoint for messaging telemetry.\",\"inputSchema\":{\"type\":\"object\",\"properties\":{}},\"annotations\":{\"readOnlyHint\":true,\"destructiveHint\":false,\"idempotentHint\":true,\"openWorldHint\":false}},"
  <> "{\"name\":\"trace_backends\",\"title\":\"Trace backends\",\"description\":\"Read Tempo readiness and Jaeger service discovery to confirm OTLP trace export/query wiring.\",\"inputSchema\":{\"type\":\"object\",\"properties\":{}},\"annotations\":{\"readOnlyHint\":true,\"destructiveHint\":false,\"idempotentHint\":true,\"openWorldHint\":false}}"
  <> "]}}"
}

fn tools_call_result(tool: String, request_id: String) -> String {
  case tool {
    "kubernetes_inventory" ->
      tool_json_result(
        request_id,
        "kubernetes_inventory",
        "Bounded Kubernetes cluster inventory metadata visible to the read-only MCP service account.",
        k8s.inventory_json(),
      )
    "kubernetes_deployments" ->
      tool_json_result(
        request_id,
        "kubernetes_deployments",
        "All Kubernetes deployments visible to the read-only MCP service account.",
        k8s.deployments_json(),
      )
    "human_access_policy" ->
      tool_json_result(
        request_id,
        "human_access_policy",
        "Human-authenticated access policy for the DD runtime gateway, MCP, VPN, and bastion.",
        k8s.human_access_policy_json(),
      )
    "telemetry_targets" ->
      tool_json_result(
        request_id,
        "telemetry_targets",
        "In-cluster observability endpoints and safe read-only queries.",
        observability.targets_json(),
      )
    "telemetry_summary" ->
      tool_json_result(
        request_id,
        "telemetry_summary",
        "Bounded parallel telemetry summary from the in-cluster observability and NATS endpoints.",
        observability.telemetry_summary_json(),
      )
    "observability_health" ->
      tool_json_result(
        request_id,
        "observability_health",
        "Live bounded health checks for Prometheus, Loki, Grafana, Tempo, Jaeger, and the OTel collector.",
        observability.health_json(),
      )
    "prometheus_up" ->
      tool_json_result(
        request_id,
        "prometheus_up",
        "Prometheus instant query `up` returned from the in-cluster Prometheus API.",
        observability.prometheus_up_json(),
      )
    "loki_labels" ->
      tool_json_result(
        request_id,
        "loki_labels",
        "Loki label names returned from the in-cluster Loki API.",
        observability.loki_labels_json(),
      )
    "grafana_inventory" ->
      tool_json_result(
        request_id,
        "grafana_inventory",
        "Grafana datasource and dashboard inventory returned from the in-cluster Grafana API.",
        observability.grafana_inventory_json(),
      )
    "nats_metrics" ->
      tool_json_result(
        request_id,
        "nats_metrics",
        "NATS /varz and Prometheus exporter metrics returned from the in-cluster messaging service.",
        observability.nats_metrics_json(),
      )
    "trace_backends" ->
      tool_json_result(
        request_id,
        "trace_backends",
        "Tempo readiness and Jaeger service discovery returned from in-cluster trace backends.",
        observability.trace_backends_json(),
      )
    "service_directory" -> service_directory_result(request_id)
    "cluster_status" -> cluster_status_result(request_id)
    _ -> json_rpc_error_with_id("unknown tool", -32_602, request_id)
  }
}

fn tool_json_result(
  request_id: String,
  tool: String,
  text: String,
  structured_content: String,
) -> String {
  "{\"jsonrpc\":\"2.0\",\"id\":"
  <> request_id
  <> ",\"result\":{\"content\":[{\"type\":\"text\",\"text\":\""
  <> json_escape(text)
  <> "\"}],\"structuredContent\":"
  <> structured_content
  <> ",\"_meta\":{\"tool\":\""
  <> tool
  <> "\"}}}"
}

fn cluster_status_result(request_id: String) -> String {
  tool_json_result(
    request_id,
    "cluster_status",
    "DD remote Kubernetes runtime MCP status.",
    "{\"service\":\"dd-gleam-mcp-server\",\"namespace\":\"default\",\"language\":\"gleam\",\"runtime\":\"beam\",\"gatewayPath\":\"/mcp\",\"metricsPath\":\"/mcp/metrics\",\"observability\":{\"grafana\":\"/telemetry/\",\"prometheus\":\"/prometheus/\",\"loki\":\"dd-loki.observability.svc.cluster.local:3100\",\"tempo\":\"dd-tempo.observability.svc.cluster.local:3200\",\"jaeger\":\"dd-jaeger.observability.svc.cluster.local:16686\",\"otelCollector\":\"dd-otel-collector.observability.svc.cluster.local:8889\",\"natsMonitor\":\"dd-nats.messaging.svc.cluster.local:8222\",\"natsMetrics\":\"dd-nats.messaging.svc.cluster.local:7777\"}}",
  )
}

fn service_directory_result(request_id: String) -> String {
  tool_json_result(
    request_id,
    "service_directory",
    "Gateway and observability service directory.",
    "{\"public\":[\"/mcp\",\"/mcp/home\",\"/mcp/healthz\",\"/mcp/metrics\",\"/telemetry/\",\"/prometheus/\",\"/nats/\",\"/nats-metrics/metrics\"],\"internal\":[\"dd-prometheus.observability.svc.cluster.local:9090\",\"dd-loki.observability.svc.cluster.local:3100\",\"dd-grafana.observability.svc.cluster.local:3000\",\"dd-tempo.observability.svc.cluster.local:3200\",\"dd-jaeger.observability.svc.cluster.local:16686\",\"dd-otel-collector.observability.svc.cluster.local:4317\",\"dd-otel-collector.observability.svc.cluster.local:4318\",\"dd-otel-collector.observability.svc.cluster.local:8889\",\"dd-nats.messaging.svc.cluster.local:8222\",\"dd-nats.messaging.svc.cluster.local:7777\"]}",
  )
}

fn json_rpc_error(message: String, code: Int) -> String {
  json_rpc_error_with_id(message, code, "1")
}

fn json_rpc_error_with_id(
  message: String,
  code: Int,
  request_id: String,
) -> String {
  "{\"jsonrpc\":\"2.0\",\"id\":"
  <> request_id
  <> ",\"error\":{\"code\":"
  <> int.to_string(code)
  <> ",\"message\":\""
  <> message
  <> "\"}}"
}

fn redirect(path: String) -> response.Response(mist.ResponseData) {
  response.new(302)
  |> response.set_header("location", path)
  |> response.set_body(mist.Bytes(bytes_tree.from_string("")))
}

fn home_page() -> response.Response(mist.ResponseData) {
  response.new(200)
  |> response.set_header("content-type", "text/html; charset=utf-8")
  |> response.set_body(mist.Bytes(bytes_tree.from_string(home_html)))
}

fn mcp_info() -> response.Response(mist.ResponseData) {
  json_response(
    200,
    "{\"ok\":true,\"service\":\"dd-gleam-mcp-server\",\"protocolVersion\":\""
      <> protocol_version
      <> "\",\"endpoint\":\"POST /mcp\",\"tools\":[\"cluster_status\",\"service_directory\",\"kubernetes_inventory\",\"kubernetes_deployments\",\"human_access_policy\",\"telemetry_targets\",\"telemetry_summary\",\"observability_health\",\"prometheus_up\",\"loki_labels\",\"grafana_inventory\",\"nats_metrics\",\"trace_backends\"]}",
  )
}

fn mcp_get(
  req: request.Request(mist.Connection),
) -> response.Response(mist.ResponseData) {
  case request.get_header(req, "accept") {
    Ok(value) ->
      case string.contains(string.lowercase(value), "text/event-stream") {
        True -> empty_response(405)
        False -> mcp_info()
      }
    Error(_) -> mcp_info()
  }
}

fn healthz() -> response.Response(mist.ResponseData) {
  json_response(
    200,
    "{\"ok\":true,\"service\":\"dd-gleam-mcp-server\",\"mode\":\"mcp\"}",
  )
}

fn observability_response() -> response.Response(mist.ResponseData) {
  json_response(200, observability.telemetry_summary_json())
}

fn metrics_response(
  metrics_name: process.Name(metrics.Message),
) -> response.Response(mist.ResponseData) {
  let metrics_subject = process.named_subject(metrics_name)
  let snapshot = process.call(metrics_subject, 1000, metrics.GetSnapshot)
  let metrics.Snapshot(
    http_requests: http_requests,
    rpc_requests: rpc_requests,
    initialize_requests: initialize_requests,
    tools_list_requests: tools_list_requests,
    tools_call_requests: tools_call_requests,
    ping_requests: ping_requests,
    unknown_requests: unknown_requests,
  ) = snapshot

  response.new(200)
  |> response.set_header(
    "content-type",
    "text/plain; version=0.0.4; charset=utf-8",
  )
  |> response.set_body(
    mist.Bytes(bytes_tree.from_string(
      "# HELP dd_gleam_mcp_http_requests_total HTTP requests observed by the Gleam MCP server.\n"
      <> "# TYPE dd_gleam_mcp_http_requests_total counter\n"
      <> "dd_gleam_mcp_http_requests_total{service=\"dd-gleam-mcp-server\"} "
      <> int.to_string(http_requests)
      <> "\n# HELP dd_gleam_mcp_rpc_requests_total JSON-RPC requests observed by method.\n"
      <> "# TYPE dd_gleam_mcp_rpc_requests_total counter\n"
      <> "dd_gleam_mcp_rpc_requests_total{service=\"dd-gleam-mcp-server\",method=\"all\"} "
      <> int.to_string(rpc_requests)
      <> "\ndd_gleam_mcp_rpc_requests_total{service=\"dd-gleam-mcp-server\",method=\"initialize\"} "
      <> int.to_string(initialize_requests)
      <> "\ndd_gleam_mcp_rpc_requests_total{service=\"dd-gleam-mcp-server\",method=\"tools/list\"} "
      <> int.to_string(tools_list_requests)
      <> "\ndd_gleam_mcp_rpc_requests_total{service=\"dd-gleam-mcp-server\",method=\"tools/call\"} "
      <> int.to_string(tools_call_requests)
      <> "\ndd_gleam_mcp_rpc_requests_total{service=\"dd-gleam-mcp-server\",method=\"ping\"} "
      <> int.to_string(ping_requests)
      <> "\ndd_gleam_mcp_rpc_requests_total{service=\"dd-gleam-mcp-server\",method=\"unknown\"} "
      <> int.to_string(unknown_requests)
      <> "\n",
    )),
  )
}

fn json_response(
  status: Int,
  body: String,
) -> response.Response(mist.ResponseData) {
  response.new(status)
  |> response.set_header("content-type", "application/json")
  |> response.set_header("mcp-protocol-version", protocol_version)
  |> response.set_body(mist.Bytes(bytes_tree.from_string(body)))
}

fn empty_response(status: Int) -> response.Response(mist.ResponseData) {
  response.new(status)
  |> response.set_header("mcp-protocol-version", protocol_version)
  |> response.set_body(mist.Bytes(bytes_tree.from_string("")))
}

fn not_found() -> response.Response(mist.ResponseData) {
  json_response(404, "{\"error\":\"not-found\"}")
}

fn record_http(metrics_name: process.Name(metrics.Message)) -> Nil {
  let metrics_subject = process.named_subject(metrics_name)
  process.send(metrics_subject, metrics.RecordHttpRequest)
}

fn record_rpc(
  metrics_name: process.Name(metrics.Message),
  method: String,
) -> Nil {
  let metrics_subject = process.named_subject(metrics_name)
  process.send(metrics_subject, metrics.RecordRpcRequest(method))
}

fn json_escape(input: String) -> String {
  input
  |> string.replace("\\", "\\\\")
  |> string.replace("\"", "\\\"")
  |> string.replace("\n", "\\n")
  |> string.replace("\r", "\\r")
  |> string.replace("\t", "\\t")
}

const home_html = "<!doctype html><html><head><meta charset=\"utf-8\"/><title>dd gleam MCP server</title><style>body{font-family:system-ui;margin:24px;line-height:1.5}code,pre{background:#111;color:#d7fbf4;border-radius:8px}code{padding:2px 5px}pre{padding:12px;overflow:auto}a{color:#047857}</style></head><body><h1>dd gleam MCP server</h1><p>Dedicated MCP deployment for the DD remote Kubernetes runtime.</p><ul><li>JSON-RPC endpoint: <code id=\"mcp-path\">/mcp</code></li><li>Health: <code id=\"health-path\">/healthz</code></li><li>Prometheus metrics: <code id=\"metrics-path\">/metrics</code></li><li>Grafana: <a href=\"/telemetry/\">/telemetry/</a></li></ul><pre>{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}</pre><script>const prefix=location.pathname.startsWith('/mcp/')?'/mcp':'';document.getElementById('mcp-path').textContent=prefix||'/mcp';document.getElementById('health-path').textContent=prefix+'/healthz';document.getElementById('metrics-path').textContent=prefix+'/metrics';</script></body></html>"
