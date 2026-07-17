import dd_otel_client
import dd_runtime_config_client
import gleam/bit_array
import gleam/bytes_tree
import gleam/http.{Get, Post}
import gleam/http/request
import gleam/http/response
import gleam/int
import gleam/list
import gleam/string
import gleam_lambda_runner/api_docs
import gleam_lambda_runner/child_process
import gleam_lambda_runner/workflow
import mist

@external(erlang, "lambda_runtime_env", "getenv")
fn env_get(name: String) -> String

const default_host = "0.0.0.0"

const default_port = 8083

const max_body_bytes = 5_242_880

const default_command = "env -i PATH=\"$PATH\" NODE_ENV=production NODE_NO_WARNINGS=1 NATS_URL=\"${NATS_URL:-}\" CONTAINER_POOL_NATS_URL=\"${CONTAINER_POOL_NATS_URL:-}\" CONTAINER_POOL_NATS_SUBJECT_PREFIX=\"${CONTAINER_POOL_NATS_SUBJECT_PREFIX:-dd.remote.container_pool}\" CONTAINER_POOL_NATS_TIMEOUT_MS=\"${CONTAINER_POOL_NATS_TIMEOUT_MS:-30000}\" node --permission --allow-net --allow-fs-read=child-runtimes --allow-fs-read=../../libs/nats/subject-defs/generated/javascript child-runtimes/js-function-runner.mjs"

const child_idle_ms = 300_000

const child_timeout_ms = 30_000

pub fn supervised() {
  mist.new(dd_otel_client.trace(route))
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
) -> response.Response(mist.ResponseData) {
  case req.method, request.path_segments(req) {
    Get, [] -> redirect("/home")
    Get, ["home"] -> home_page()
    Get, ["docs", "api"] -> api_docs.html()
    Get, ["api", "docs"] -> api_docs.html()
    Get, ["api", "docs.json"] -> api_docs.json()
    Get, ["healthz"] -> healthz()
    Get, ["metrics"] -> metrics()
    Post, ["invoke", function_id] ->
      require_authenticated_post(req, fn() { invoke(req, function_id) })
    Post, ["check"] -> require_authenticated_post(req, fn() { check(req) })
    Post, ["destroy", reuse_key] ->
      require_authenticated_post(req, fn() { destroy(reuse_key) })
    Post, ["workflows", "start"] ->
      require_authenticated_post(req, fn() { workflow_start(req) })
    Get, ["workflows", "runs"] ->
      require_authenticated(req, fn() { workflow_list(req) })
    Get, ["workflows", "runs", run_id] ->
      require_authenticated(req, fn() { workflow_get(run_id) })
    Post, ["workflows", "runs", run_id, "signal"] ->
      require_authenticated_post(req, fn() { workflow_signal(req, run_id) })
    Post, ["workflows", "runs", run_id, "cancel"] ->
      require_authenticated_post(req, fn() { workflow_cancel(run_id) })
    Get, ["internal", "runtime-config"] ->
      dd_runtime_config_client.handle_snapshot(req)
    Post, ["internal", "update-runtime-config"] ->
      dd_runtime_config_client.handle_apply(req)
    Post, ["internal", "runtime-config", "reset"] ->
      dd_runtime_config_client.handle_reset(req)
    _, ["invoke", _] -> method_not_allowed()
    _, ["check"] -> method_not_allowed()
    _, ["destroy", _] -> method_not_allowed()
    _, ["workflows", "start"] -> method_not_allowed()
    _, ["internal", "update-runtime-config"] -> method_not_allowed()
    _, ["internal", "runtime-config", "reset"] -> method_not_allowed()
    _, _ -> not_found()
  }
}

fn invoke(
  req: request.Request(mist.Connection),
  function_id: String,
) -> response.Response(mist.ResponseData) {
  case mist.read_body(req, max_body_bytes) {
    Ok(req) -> {
      case bit_array.to_string(req.body) {
        Ok(payload) -> {
          case
            child_process.invoke(
              default_command,
              function_id,
              request_payload(payload),
              child_idle_ms,
              child_timeout_ms,
            )
          {
            Ok(output) ->
              json_response(
                200,
                "{\"ok\":true,\"output\":\"" <> json_escape(output) <> "\"}",
              )

            Error(error) ->
              json_response(
                502,
                "{\"ok\":false,\"error\":\"" <> json_escape(error) <> "\"}",
              )
          }
        }
        Error(_) ->
          json_response(400, "{\"ok\":false,\"error\":\"body-not-utf8\"}")
      }
    }
    Error(_) -> json_response(400, "{\"ok\":false,\"error\":\"invalid-body\"}")
  }
}

fn check(
  req: request.Request(mist.Connection),
) -> response.Response(mist.ResponseData) {
  case mist.read_body(req, max_body_bytes) {
    Ok(req) -> {
      case bit_array.to_string(req.body) {
        Ok(payload) -> {
          case
            child_process.check_definition(
              default_command,
              request_payload(payload),
              child_timeout_ms,
            )
          {
            Ok(output) -> {
              let status = case string.contains(output, "\"ok\":false") {
                True -> 422
                False -> 200
              }
              json_response(status, output)
            }

            Error(error) ->
              json_response(
                502,
                "{\"ok\":false,\"error\":\"" <> json_escape(error) <> "\"}",
              )
          }
        }
        Error(_) ->
          json_response(400, "{\"ok\":false,\"error\":\"body-not-utf8\"}")
      }
    }
    Error(_) -> json_response(400, "{\"ok\":false,\"error\":\"invalid-body\"}")
  }
}

fn destroy(reuse_key: String) -> response.Response(mist.ResponseData) {
  case child_process.destroy(reuse_key) {
    Ok(message) ->
      json_response(
        200,
        "{\"ok\":true,\"message\":\"" <> json_escape(message) <> "\"}",
      )

    Error(error) ->
      json_response(
        502,
        "{\"ok\":false,\"error\":\"" <> json_escape(error) <> "\"}",
      )
  }
}

fn workflow_start(
  req: request.Request(mist.Connection),
) -> response.Response(mist.ResponseData) {
  with_body(req, fn(payload) {
    workflow_result_response(201, "run", workflow.start_run(payload))
  })
}

fn workflow_signal(
  req: request.Request(mist.Connection),
  run_id: String,
) -> response.Response(mist.ResponseData) {
  with_body(req, fn(payload) {
    workflow_result_response(200, "run", workflow.signal_run(run_id, payload))
  })
}

fn workflow_cancel(run_id: String) -> response.Response(mist.ResponseData) {
  workflow_result_response(200, "run", workflow.cancel_run(run_id))
}

fn workflow_get(run_id: String) -> response.Response(mist.ResponseData) {
  // get_run already returns a wrapped {"ok":true,"run":...,"steps":...} body.
  case workflow.get_run(run_id) {
    Ok(body) -> json_response(200, body)
    Error(error) -> workflow_error_response(error)
  }
}

fn workflow_list(
  req: request.Request(mist.Connection),
) -> response.Response(mist.ResponseData) {
  let definition = query_value(req, "definition")
  let limit = case int.parse(query_value(req, "limit")) {
    Ok(value) -> value
    Error(_) -> 100
  }
  case workflow.list_runs(definition, limit) {
    Ok(runs) ->
      json_response(200, "{\"ok\":true,\"runs\":" <> runs <> "}")
    Error(error) -> workflow_error_response(error)
  }
}

fn workflow_result_response(
  ok_status: Int,
  key: String,
  result: Result(String, String),
) -> response.Response(mist.ResponseData) {
  case result {
    Ok(body) ->
      json_response(
        ok_status,
        "{\"ok\":true,\"" <> key <> "\":" <> body <> "}",
      )
    Error(error) -> workflow_error_response(error)
  }
}

fn workflow_error_response(
  error: String,
) -> response.Response(mist.ResponseData) {
  let status = workflow_error_status(error)
  json_response(
    status,
    "{\"ok\":false,\"error\":\"" <> json_escape(error) <> "\"}",
  )
}

fn workflow_error_status(error: String) -> Int {
  case string.contains(error, "not found") {
    True -> 404
    False ->
      case
        string.contains(error, "not cancelable")
        || string.contains(error, "not running")
      {
        True -> 409
        False ->
          case
            string.contains(error, "required")
            || string.contains(error, "invalid")
            || string.contains(error, "must")
            || string.contains(error, "not active")
          {
            True -> 400
            False -> 502
          }
      }
  }
}

fn with_body(
  req: request.Request(mist.Connection),
  next: fn(String) -> response.Response(mist.ResponseData),
) -> response.Response(mist.ResponseData) {
  case mist.read_body(req, max_body_bytes) {
    Ok(req) ->
      case bit_array.to_string(req.body) {
        Ok(payload) -> next(payload)
        Error(_) ->
          json_response(400, "{\"ok\":false,\"error\":\"body-not-utf8\"}")
      }
    Error(_) -> json_response(400, "{\"ok\":false,\"error\":\"invalid-body\"}")
  }
}

fn query_value(req: request.Request(mist.Connection), key: String) -> String {
  case request.get_query(req) {
    Ok(pairs) ->
      case list.key_find(pairs, key) {
        Ok(value) -> value
        Error(_) -> ""
      }
    Error(_) -> ""
  }
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

fn healthz() -> response.Response(mist.ResponseData) {
  json_response(
    200,
    "{\"ok\":true,\"service\":\"dd-gleam-lambda-runner\",\"authConfigured\":"
      <> bool_json(server_auth_configured())
      <> ",\"postgresConfigured\":"
      <> bool_json(env_get("LAMBDA_DATABASE_URL") != "")
      <> ",\"natsConfigured\":"
      <> bool_json(env_get("NATS_URL") != "")
      <> ",\"workflowEngineEnabled\":"
      <> bool_json(workflow.enabled())
      <> "}",
  )
}

fn metrics() -> response.Response(mist.ResponseData) {
  response.new(200)
  |> response.set_header(
    "content-type",
    "text/plain; version=0.0.4; charset=utf-8",
  )
  |> response.set_body(
    mist.Bytes(bytes_tree.from_string(
      child_process.metrics() <> "\n" <> workflow.metrics(),
    )),
  )
}

fn not_found() -> response.Response(mist.ResponseData) {
  json_response(404, "{\"ok\":false,\"error\":\"not-found\"}")
}

fn method_not_allowed() -> response.Response(mist.ResponseData) {
  response.new(405)
  |> response.set_header("allow", "POST")
  |> response.set_header("content-type", "application/json")
  |> response.set_body(
    mist.Bytes(bytes_tree.from_string(
      "{\"ok\":false,\"error\":\"method-not-allowed\"}",
    )),
  )
}

fn unauthorized() -> response.Response(mist.ResponseData) {
  json_response(401, "{\"ok\":false,\"error\":\"unauthorized\"}")
}

fn auth_not_configured() -> response.Response(mist.ResponseData) {
  json_response(
    503,
    "{\"ok\":false,\"error\":\"SERVER_AUTH_SECRET is not configured\"}",
  )
}

fn require_post(
  req: request.Request(mist.Connection),
  next: fn() -> response.Response(mist.ResponseData),
) -> response.Response(mist.ResponseData) {
  case req.method {
    Post -> next()
    _ -> method_not_allowed()
  }
}

fn require_authenticated_post(
  req: request.Request(mist.Connection),
  next: fn() -> response.Response(mist.ResponseData),
) -> response.Response(mist.ResponseData) {
  require_post(req, fn() { require_authenticated(req, next) })
}

fn require_authenticated(
  req: request.Request(mist.Connection),
  next: fn() -> response.Response(mist.ResponseData),
) -> response.Response(mist.ResponseData) {
  let secret = server_auth_secret()
  case secret {
    "" -> auth_not_configured()
    _ -> {
      case request_is_authorized(req, secret) {
        True -> next()
        False -> unauthorized()
      }
    }
  }
}

fn server_auth_secret() -> String {
  case env_get("LAMBDA_SERVER_AUTH_SECRET") {
    "" -> {
      case env_get("SERVER_AUTH_SECRET") {
        "" -> env_get("REMOTE_DEV_SERVER_SECRET")
        value -> value
      }
    }
    value -> value
  }
}

fn server_auth_configured() -> Bool {
  server_auth_secret() != ""
}

fn request_is_authorized(
  req: request.Request(mist.Connection),
  secret: String,
) -> Bool {
  case request.get_header(req, "x-server-auth") {
    Ok(value) -> value == secret
    Error(_) -> {
      case request.get_header(req, "x-lambda-runner-auth") {
        Ok(value) -> value == secret
        Error(_) -> {
          case request.get_header(req, "x-agent-auth") {
            Ok(value) -> value == secret
            Error(_) -> False
          }
        }
      }
    }
  }
}

fn bool_json(value: Bool) -> String {
  case value {
    True -> "true"
    False -> "false"
  }
}

fn json_response(
  status: Int,
  body: String,
) -> response.Response(mist.ResponseData) {
  response.new(status)
  |> response.set_header("content-type", "application/json")
  |> response.set_body(mist.Bytes(bytes_tree.from_string(body)))
}

fn json_escape(input: String) -> String {
  input
  |> string.replace("\\", "\\\\")
  |> string.replace("\"", "\\\"")
  |> string.replace("\n", "\\n")
  |> string.replace("\r", "\\r")
  |> string.replace("\t", "\\t")
}

fn request_payload(request_payload: String) -> String {
  let payload = string.trim(request_payload)
  case payload {
    "" -> "null"
    value -> value
  }
}

const home_html = "<!doctype html><html><head><meta charset=\"utf-8\"/><title>dd gleam lambda runner</title><style>body{font-family:system-ui;margin:24px;line-height:1.45}code{background:#f1f5f9;padding:2px 5px;border-radius:4px}pre{max-height:50vh;overflow:auto;background:#111827;color:#d1fae5;padding:12px;border-radius:8px}</style></head><body><h1>dd gleam lambda runner</h1><p>Health: <code>/healthz</code></p><p>Metrics: <code>/metrics</code></p><p>Invocation endpoint: <code>POST /invoke/:function_id</code>. Gateway invocation traffic lands here directly; the child runner loads the active function definition from Postgres and Gleam manages reusable child processes.</p></body></html>"
