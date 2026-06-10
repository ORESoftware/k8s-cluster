//// Regression tests for the JSON-RPC request parser (gleam_mcp_json.erl).
////
//// These pin the hardening that replaced the old substring/regex router:
//// routing now reads the *top-level* JSON-RPC `method`, `id`, and
//// `params.name`, so request *arguments* that happen to contain a method
//// or tool literal can no longer misroute the call, and an `id` nested in
//// `params` can no longer be echoed as the response id.

import gleeunit/should

@external(erlang, "gleam_mcp_json", "parse_request")
fn parse_request(body: String) -> #(String, String, String, String)

pub fn tools_call_routes_by_method_test() {
  let #(status, method, id, tool) =
    parse_request(
      "{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"tools/call\",\"params\":{\"name\":\"kubernetes_inventory\",\"arguments\":{}}}",
    )
  should.equal(status, "request")
  should.equal(method, "tools/call")
  should.equal(id, "7")
  should.equal(tool, "kubernetes_inventory")
}

/// The headline fix: a quoted `"tools/call"` sitting inside an argument
/// value must NOT hijack a `tools/list` request. The old substring router
/// did exactly that.
pub fn quoted_method_literal_in_args_does_not_misroute_test() {
  let #(status, method, _id, _tool) =
    parse_request(
      "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\",\"params\":{\"q\":\"tools/call\"}}",
    )
  should.equal(status, "request")
  should.equal(method, "tools/list")
}

/// Same class of bug for the tool name: an argument value mentioning
/// `kubernetes_inventory` must not override the real `params.name`.
pub fn tool_literal_in_args_does_not_misroute_tool_test() {
  let #(_status, _method, _id, tool) =
    parse_request(
      "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"cluster_status\",\"arguments\":{\"ref\":\"kubernetes_inventory\"}}}",
    )
  should.equal(tool, "cluster_status")
}

pub fn string_id_is_echoed_verbatim_test() {
  let #(_status, _method, id, _tool) =
    parse_request("{\"jsonrpc\":\"2.0\",\"id\":\"abc-1\",\"method\":\"ping\"}")
  should.equal(id, "\"abc-1\"")
}

/// An `id` that only appears inside `params` is not the request id; with no
/// top-level id this is a notification (no response body).
pub fn nested_id_is_ignored_test() {
  let #(status, method, id, _tool) =
    parse_request(
      "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\",\"params\":{\"id\":99}}",
    )
  should.equal(status, "notification")
  should.equal(method, "notifications/initialized")
  should.equal(id, "null")
}

pub fn request_with_id_is_request_test() {
  let #(status, method, _id, _tool) =
    parse_request("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}")
  should.equal(status, "request")
  should.equal(method, "initialize")
}

/// MCP 2025-11-25 removed JSON-RPC batching; a top-level array is rejected
/// as invalid_request with a null id rather than fanned out.
pub fn batch_is_invalid_request_test() {
  let #(status, _method, id, _tool) =
    parse_request("[{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}]")
  should.equal(status, "invalid_request")
  should.equal(id, "null")
}

pub fn malformed_json_is_parse_error_test() {
  let #(status, _method, _id, _tool) = parse_request("{not json")
  should.equal(status, "parse_error")
}

pub fn bare_scalar_is_invalid_request_test() {
  let #(status, _method, _id, _tool) = parse_request("42")
  should.equal(status, "invalid_request")
}

pub fn object_without_method_is_invalid_request_test() {
  let #(status, _method, _id, _tool) =
    parse_request("{\"jsonrpc\":\"2.0\",\"id\":1}")
  should.equal(status, "invalid_request")
}
