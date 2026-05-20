import gleam/bit_array
import gleam/bytes_tree
import gleam/http
import gleam/http/request
import gleam/http/response
import gleam/int
import gleam/string
import gleam_lambda_runner/child_process
import mist

@external(erlang, "lambda_runtime_env", "getenv")
fn env_get(name: String) -> String

const default_host = "0.0.0.0"

const default_port = 8083

const max_body_bytes = 5_242_880

const default_command = "env -i PATH=\"$PATH\" NODE_ENV=production NODE_NO_WARNINGS=1 NATS_URL=\"${NATS_URL:-}\" CONTAINER_POOL_NATS_URL=\"${CONTAINER_POOL_NATS_URL:-}\" CONTAINER_POOL_NATS_SUBJECT_PREFIX=\"${CONTAINER_POOL_NATS_SUBJECT_PREFIX:-dd.remote.container_pool}\" CONTAINER_POOL_NATS_TIMEOUT_MS=\"${CONTAINER_POOL_NATS_TIMEOUT_MS:-30000}\" node --permission --allow-net child-runtimes/js-function-runner.mjs"

const child_idle_ms = 300_000

const child_timeout_ms = 30_000

pub fn supervised() {
  mist.new(route)
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
  case request.path_segments(req) {
    [] -> redirect("/home")
    ["home"] -> home_page()
    ["healthz"] -> healthz()
    ["metrics"] -> metrics()
    ["invoke", function_id] ->
      require_authenticated_post(req, fn() { invoke(req, function_id) })
    ["check"] -> require_authenticated_post(req, fn() { check(req) })
    ["destroy", reuse_key] ->
      require_authenticated_post(req, fn() { destroy(reuse_key) })
    _ -> not_found()
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
    mist.Bytes(bytes_tree.from_string(child_process.metrics())),
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
    http.Post -> next()
    _ -> method_not_allowed()
  }
}

fn require_authenticated_post(
  req: request.Request(mist.Connection),
  next: fn() -> response.Response(mist.ResponseData),
) -> response.Response(mist.ResponseData) {
  require_post(req, fn() {
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
  })
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
