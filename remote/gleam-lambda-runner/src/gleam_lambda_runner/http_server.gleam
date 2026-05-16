import gleam/bit_array
import gleam/bytes_tree
import gleam/http
import gleam/http/request
import gleam/http/response
import gleam/string
import gleam_lambda_runner/child_process
import mist

const host = "0.0.0.0"

const port = 8083

const max_body_bytes = 5_242_880

const default_command =
  "env -i PATH=\"$PATH\" NODE_ENV=production node --permission --allow-net child-runtimes/js-function-runner.mjs"

const child_idle_ms = 300_000

const child_timeout_ms = 30_000

pub fn supervised() {
  mist.new(route)
  |> mist.bind(host)
  |> mist.port(port)
  |> mist.supervised
}

fn route(
  req: request.Request(mist.Connection),
) -> response.Response(mist.ResponseData) {
  case request.path_segments(req) {
    [] -> redirect("/home")
    ["home"] -> home_page()
    ["healthz"] -> healthz()
    ["metrics"] -> metrics()
    ["invoke", function_id] -> require_post(req, fn() { invoke(req, function_id) })
    ["destroy", reuse_key] -> require_post(req, fn() { destroy(reuse_key) })
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
          case child_process.invoke(
            default_command,
            function_id,
            request_payload(payload),
            child_idle_ms,
            child_timeout_ms,
          ) {
            Ok(output) ->
              json_response(200, "{\"ok\":true,\"output\":\"" <> json_escape(output) <> "\"}")

            Error(error) ->
              json_response(502, "{\"ok\":false,\"error\":\"" <> json_escape(error) <> "\"}")
          }
        }
        Error(_) -> json_response(400, "{\"ok\":false,\"error\":\"body-not-utf8\"}")
      }
    }
    Error(_) -> json_response(400, "{\"ok\":false,\"error\":\"invalid-body\"}")
  }
}

fn destroy(reuse_key: String) -> response.Response(mist.ResponseData) {
  case child_process.destroy(reuse_key) {
    Ok(message) ->
      json_response(200, "{\"ok\":true,\"message\":\"" <> json_escape(message) <> "\"}")

    Error(error) ->
      json_response(502, "{\"ok\":false,\"error\":\"" <> json_escape(error) <> "\"}")
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
  json_response(200, "{\"ok\":true,\"service\":\"dd-gleam-lambda-runner\"}")
}

fn metrics() -> response.Response(mist.ResponseData) {
  response.new(200)
  |> response.set_header(
    "content-type",
    "text/plain; version=0.0.4; charset=utf-8",
  )
  |> response.set_body(mist.Bytes(bytes_tree.from_string(child_process.metrics())))
}

fn not_found() -> response.Response(mist.ResponseData) {
  json_response(404, "{\"ok\":false,\"error\":\"not-found\"}")
}

fn method_not_allowed() -> response.Response(mist.ResponseData) {
  response.new(405)
  |> response.set_header("allow", "POST")
  |> response.set_header("content-type", "application/json")
  |> response.set_body(mist.Bytes(bytes_tree.from_string("{\"ok\":false,\"error\":\"method-not-allowed\"}")))
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
}

fn request_payload(request_payload: String) -> String {
  let payload = string.trim(request_payload)
  case payload {
    "" -> "null"
    value -> value
  }
}

const home_html = "<!doctype html><html><head><meta charset=\"utf-8\"/><title>dd gleam lambda runner</title><style>body{font-family:system-ui;margin:24px;line-height:1.45}code{background:#f1f5f9;padding:2px 5px;border-radius:4px}pre{max-height:50vh;overflow:auto;background:#111827;color:#d1fae5;padding:12px;border-radius:8px}</style></head><body><h1>dd gleam lambda runner</h1><p>Health: <code>/healthz</code></p><p>Metrics: <code>/metrics</code></p><p>Invocation endpoint: <code>POST /invoke/:function_id</code>. Gateway invocation traffic lands here directly; the child runner loads the active function definition from Postgres and Gleam manages reusable child processes.</p></body></html>"
