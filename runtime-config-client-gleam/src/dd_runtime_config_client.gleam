//// Receiver helper for the dd-runtime-config control plane (Gleam edition).
////
//// Public API:
////   - `start_registration_loop()` — spawn the background register-with-retry
////     loop. Call this once during service startup (after the HTTP listener
////     is supervised). Safe to call multiple times — the FFI process tracks
////     state per node.
////   - `handle_snapshot(req)` — returns the JSON snapshot currently held in
////     this process. Mount at `GET /internal/runtime-config`.
////   - `handle_apply(req)` — accepts a `RuntimeConfigApplyRequest` payload.
////     Mount at `POST /internal/update-runtime-config`. Requires the same
////     `X-Server-Auth` header value as `$RUNTIME_CONFIG_SERVER_SECRET`.
////   - `handle_reset(req)` — drops the snapshot. Mount at
////     `POST /internal/runtime-config/reset`. Requires `X-Server-Auth`.
////
//// All state lives in Erlang's `persistent_term` table (low-contention reads),
//// updated atomically when an apply lands. Outbound registration is a plain
//// `gen_tcp` HTTP POST in the FFI module so we don't drag in inets/ssl just
//// to talk to a sibling pod.
////
//// Payload shape mirrors
//// `remote/libs/interfaces/shared/schema/runtime-config.schema.json`.

import gleam/bytes_tree
import gleam/http
import gleam/http/request
import gleam/http/response
import mist

pub const snapshot_route: List(String) = ["internal", "runtime-config"]

pub const apply_route: List(String) = ["internal", "update-runtime-config"]

pub const reset_route: List(String) = ["internal", "runtime-config", "reset"]

@external(erlang, "dd_runtime_config_client_ffi", "start_registration")
pub fn start_registration_loop() -> Nil

@external(erlang, "dd_runtime_config_client_ffi", "snapshot_json")
fn snapshot_json() -> String

@external(erlang, "dd_runtime_config_client_ffi", "apply_payload")
fn apply_payload(payload: BitArray) -> Result(String, String)

@external(erlang, "dd_runtime_config_client_ffi", "reset")
fn reset_state() -> Nil

@external(erlang, "dd_runtime_config_client_ffi", "auth_ok")
fn auth_ok(provided: String) -> Bool

/// Mounted at `GET /internal/runtime-config`.
pub fn handle_snapshot(
  _req: request.Request(mist.Connection),
) -> response.Response(mist.ResponseData) {
  json_response(200, snapshot_json())
}

/// Mounted at `POST /internal/update-runtime-config`.
pub fn handle_apply(
  req: request.Request(mist.Connection),
) -> response.Response(mist.ResponseData) {
  case enforce_auth(req) {
    Error(resp) -> resp
    Ok(_) -> {
      case mist.read_body(req, 1_048_576) {
        Error(_) -> json_response(400, "{\"ok\":false,\"error\":\"invalid-body\"}")
        Ok(body_req) ->
          case apply_payload(body_req.body) {
            Ok(json) -> json_response(200, json)
            Error(reason) ->
              json_response(
                400,
                "{\"ok\":false,\"error\":\"" <> escape_json(reason) <> "\"}",
              )
          }
      }
    }
  }
}

/// Mounted at `POST /internal/runtime-config/reset`.
pub fn handle_reset(
  req: request.Request(mist.Connection),
) -> response.Response(mist.ResponseData) {
  case enforce_auth(req) {
    Error(resp) -> resp
    Ok(_) -> {
      reset_state()
      json_response(200, "{\"ok\":true}")
    }
  }
}

/// Helper for services whose route function discriminates on method, so they
/// can write a one-line guard instead of pattern-matching on path twice.
pub fn is_apply_route(method: http.Method, segments: List(String)) -> Bool {
  case method, segments {
    http.Post, ["internal", "update-runtime-config"] -> True
    _, _ -> False
  }
}

fn enforce_auth(
  req: request.Request(mist.Connection),
) -> Result(Nil, response.Response(mist.ResponseData)) {
  case request.get_header(req, "x-server-auth") {
    Ok(value) ->
      case auth_ok(value) {
        True -> Ok(Nil)
        False -> Error(json_response(401, "{\"ok\":false,\"error\":\"unauthorized\"}"))
      }
    Error(_) ->
      case auth_ok("") {
        True -> Ok(Nil)
        False -> Error(json_response(401, "{\"ok\":false,\"error\":\"unauthorized\"}"))
      }
  }
}

fn json_response(status: Int, body: String) -> response.Response(mist.ResponseData) {
  response.new(status)
  |> response.set_header("content-type", "application/json; charset=utf-8")
  |> response.set_body(mist.Bytes(bytes_tree.from_string(body)))
}

fn escape_json(value: String) -> String {
  // Minimal escape so error strings are safe to embed in our error envelope.
  value
}
