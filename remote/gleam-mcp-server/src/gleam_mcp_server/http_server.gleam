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
import gleam_mcp_server/metrics
import mist

const host = "0.0.0.0"

const port = 8090

const protocol_version = "2025-11-25"

pub fn supervised(metrics_name: process.Name(metrics.Message)) {
  mist.new(fn(req) { route(req, metrics_name) })
  |> mist.bind(host)
  |> mist.port(port)
  |> mist.supervised
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
    Get, ["mcp"] -> mcp_info()
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
      _ -> json_response(200, rpc_payload(method))
    }
  })
  |> result.unwrap(json_response(400, json_rpc_error("parse error", -32700)))
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

fn rpc_payload(method: String) -> String {
  case method {
    "initialize" -> initialize_result()
    "tools/list" -> tools_list_result()
    "tools/call" -> tools_call_result()
    "ping" -> "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}"
    _ -> json_rpc_error("method not found", -32601)
  }
}

fn initialize_result() -> String {
  "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":\""
  <> protocol_version
  <> "\",\"capabilities\":{\"tools\":{\"listChanged\":false}},\"serverInfo\":{\"name\":\"dd-gleam-mcp-server\",\"title\":\"DD Gleam MCP Server\",\"version\":\"0.1.0\",\"description\":\"Gleam MCP endpoint for the DD remote Kubernetes runtime\"},\"instructions\":\"Use tools/list to inspect read-only cluster runtime helpers. The service exports Prometheus metrics at /metrics and writes structured-ish request logs to stdout for Loki.\"}}"
}

fn tools_list_result() -> String {
  "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":["
  <> "{\"name\":\"cluster_status\",\"title\":\"Cluster status\",\"description\":\"Return static service discovery details for the DD remote Kubernetes runtime.\",\"inputSchema\":{\"type\":\"object\",\"properties\":{}},\"annotations\":{\"readOnlyHint\":true,\"destructiveHint\":false,\"idempotentHint\":true,\"openWorldHint\":false}},"
  <> "{\"name\":\"service_directory\",\"title\":\"Service directory\",\"description\":\"List public and internal service paths exposed by the runtime gateway.\",\"inputSchema\":{\"type\":\"object\",\"properties\":{}},\"annotations\":{\"readOnlyHint\":true,\"destructiveHint\":false,\"idempotentHint\":true,\"openWorldHint\":false}},"
  <> "{\"name\":\"telemetry_targets\",\"title\":\"Telemetry targets\",\"description\":\"List Prometheus scrape targets and dashboard paths for this runtime.\",\"inputSchema\":{\"type\":\"object\",\"properties\":{}},\"annotations\":{\"readOnlyHint\":true,\"destructiveHint\":false,\"idempotentHint\":true,\"openWorldHint\":false}}"
  <> "]}}"
}

fn tools_call_result() -> String {
  "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"dd-gleam-mcp-server is running in Kubernetes. Public gateway path: /mcp. Metrics path: /mcp/metrics. Grafana path: /telemetry/.\"}],\"structuredContent\":{\"service\":\"dd-gleam-mcp-server\",\"namespace\":\"default\",\"language\":\"gleam\",\"runtime\":\"beam\",\"gatewayPath\":\"/mcp\",\"metricsPath\":\"/mcp/metrics\",\"telemetry\":{\"prometheusJob\":\"dd-gleam-mcp-server\",\"grafanaDashboard\":\"dd-remote-dev-runtime\",\"lokiLabels\":{\"app\":\"dd-gleam-mcp-server\"}}}}}"
}

fn json_rpc_error(message: String, code: Int) -> String {
  "{\"jsonrpc\":\"2.0\",\"id\":1,\"error\":{\"code\":"
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
  json_response(200, "{\"ok\":true,\"service\":\"dd-gleam-mcp-server\",\"protocolVersion\":\"" <> protocol_version <> "\",\"endpoint\":\"POST /mcp\",\"tools\":[\"cluster_status\",\"service_directory\",\"telemetry_targets\"]}")
}

fn healthz() -> response.Response(mist.ResponseData) {
  json_response(200, "{\"ok\":true,\"service\":\"dd-gleam-mcp-server\",\"mode\":\"mcp\"}")
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
  |> response.set_body(mist.Bytes(bytes_tree.from_string(
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
  )))
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

const home_html = "<!doctype html><html><head><meta charset=\"utf-8\"/><title>dd gleam MCP server</title><style>body{font-family:system-ui;margin:24px;line-height:1.5}code,pre{background:#111;color:#d7fbf4;border-radius:8px}code{padding:2px 5px}pre{padding:12px;overflow:auto}a{color:#047857}</style></head><body><h1>dd gleam MCP server</h1><p>Dedicated MCP deployment for the DD remote Kubernetes runtime.</p><ul><li>JSON-RPC endpoint: <code id=\"mcp-path\">/mcp</code></li><li>Health: <code id=\"health-path\">/healthz</code></li><li>Prometheus metrics: <code id=\"metrics-path\">/metrics</code></li><li>Grafana: <a href=\"/telemetry/\">/telemetry/</a></li></ul><pre>{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}</pre><script>const prefix=location.pathname.startsWith('/mcp/')?'/mcp':'';document.getElementById('mcp-path').textContent=prefix||'/mcp';document.getElementById('health-path').textContent=prefix+'/healthz';document.getElementById('metrics-path').textContent=prefix+'/metrics';</script></body></html>"
