import dd_otel_client
import dd_runtime_config_client
import gleam/bit_array
import gleam/bytes_tree
import gleam/erlang/process
import gleam/http.{Delete, Get, Post}
import gleam/http/request
import gleam/http/response
import gleam/int
import gleam/list
import gleam/string
import gleam_lambda_runner/actors
import gleam_lambda_runner/api_docs
import gleam_lambda_runner/async_invocation
import gleam_lambda_runner/child_process
import gleam_lambda_runner/events
import gleam_lambda_runner/runtime_supervisor
import gleam_lambda_runner/schedule
import gleam_lambda_runner/workflow
import mist

@external(erlang, "lambda_runtime_env", "getenv")
fn env_get(name: String) -> String

const default_host = "0.0.0.0"

const default_port = 8083

const max_body_bytes = 5_242_880

const default_command = "env -i PATH=\"$PATH\" NODE_ENV=production NODE_NO_WARNINGS=1 NATS_URL=\"${NATS_URL:-}\" CONTAINER_POOL_NATS_URL=\"${CONTAINER_POOL_NATS_URL:-}\" CONTAINER_POOL_NATS_SUBJECT_PREFIX=\"${CONTAINER_POOL_NATS_SUBJECT_PREFIX:-dd.remote.container_pool}\" CONTAINER_POOL_NATS_TIMEOUT_MS=\"${CONTAINER_POOL_NATS_TIMEOUT_MS:-30000}\" LAMBDA_STREAM_MAX_BYTES=\"${LAMBDA_STREAM_MAX_BYTES:-16777216}\" LAMBDA_STREAM_CHUNK_BYTES=\"${LAMBDA_STREAM_CHUNK_BYTES:-65536}\" child-runtimes/node-permission-launcher.sh --allow-fs-read=child-runtimes --allow-fs-read=../../../../libs child-runtimes/js-function-runner.mjs"

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
    Get, ["readyz"] -> readyz()
    Get, ["metrics"] -> metrics()
    Get, ["internal", "runtime-processes"] ->
      require_authenticated(req, runtime_processes)
    Post, ["invoke", function_id, "aliases", alias] ->
      require_authenticated_post(req, fn() {
        invoke_release(req, function_id, alias)
      })
    Post, ["invoke", function_id, "revisions", revision] ->
      require_authenticated_post(req, fn() {
        invoke_release(req, function_id, revision)
      })
    Post, ["invoke", function_id] ->
      require_authenticated_post(req, fn() { invoke(req, function_id) })
    Post, ["invoke-stream", function_id, "aliases", alias] ->
      require_authenticated_post(req, fn() {
        invoke_stream_release(req, function_id, alias)
      })
    Post, ["invoke-stream", function_id, "revisions", revision] ->
      require_authenticated_post(req, fn() {
        invoke_stream_release(req, function_id, revision)
      })
    Post, ["invoke-stream", function_id] ->
      require_authenticated_post(req, fn() { invoke_stream(req, function_id) })
    Post, ["actors", function_id, actor_key] ->
      require_authenticated_post(req, fn() {
        actor_invoke(req, function_id, actor_key)
      })
    Get, ["actors", function_id, actor_key, "state"] ->
      require_authenticated(req, fn() { actor_state(function_id, actor_key) })
    Delete, ["actors", function_id, actor_key] ->
      require_authenticated(req, fn() { actor_reset(function_id, actor_key) })
    Post, ["check"] -> require_authenticated_post(req, fn() { check(req) })
    Post, ["destroy", reuse_key] ->
      require_authenticated_post(req, fn() { destroy(reuse_key) })
    Post, ["async", "invoke", function_id, "aliases", alias] ->
      require_authenticated_post(req, fn() {
        async_invoke_release(req, function_id, alias)
      })
    Post, ["async", "invoke", function_id, "revisions", revision] ->
      require_authenticated_post(req, fn() {
        async_invoke_release(req, function_id, revision)
      })
    Post, ["async", "invoke", function_id] ->
      require_authenticated_post(req, fn() { async_invoke(req, function_id) })
    Get, ["async", "invocations", run_id] ->
      require_authenticated(req, fn() { async_get(run_id) })
    Post, ["async", "invocations", run_id, "cancel"] ->
      require_authenticated_post(req, fn() { async_cancel(run_id) })
    Post, ["events"] ->
      require_authenticated_post(req, fn() { event_route(req) })
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
    _, ["invoke", _, "aliases", _] -> method_not_allowed()
    _, ["invoke", _, "revisions", _] -> method_not_allowed()
    _, ["invoke-stream", _] -> method_not_allowed()
    _, ["invoke-stream", _, "aliases", _] -> method_not_allowed()
    _, ["invoke-stream", _, "revisions", _] -> method_not_allowed()
    _, ["actors", _, _] -> method_not_allowed()
    _, ["actors", _, _, "state"] -> method_not_allowed()
    _, ["check"] -> method_not_allowed()
    _, ["destroy", _] -> method_not_allowed()
    _, ["async", "invoke", _] -> method_not_allowed()
    _, ["async", "invoke", _, "aliases", _] -> method_not_allowed()
    _, ["async", "invoke", _, "revisions", _] -> method_not_allowed()
    _, ["async", "invocations", _] -> method_not_allowed()
    _, ["async", "invocations", _, "cancel"] -> method_not_allowed()
    _, ["events"] -> method_not_allowed()
    _, ["workflows", "start"] -> method_not_allowed()
    _, ["internal", "update-runtime-config"] -> method_not_allowed()
    _, ["internal", "runtime-config", "reset"] -> method_not_allowed()
    _, _ -> not_found()
  }
}

fn actor_invoke(
  req: request.Request(mist.Connection),
  function_id: String,
  actor_key: String,
) -> response.Response(mist.ResponseData) {
  with_body(req, fn(payload) {
    case actors.invoke(function_id, actor_key, request_payload(payload)) {
      Ok(body) -> json_response(200, body)
      Error(error) -> actor_error_response(error)
    }
  })
}

fn actor_state(
  function_id: String,
  actor_key: String,
) -> response.Response(mist.ResponseData) {
  case actors.get_state(function_id, actor_key) {
    Ok(body) -> json_response(200, body)
    Error(error) -> actor_error_response(error)
  }
}

fn actor_reset(
  function_id: String,
  actor_key: String,
) -> response.Response(mist.ResponseData) {
  case actors.reset(function_id, actor_key) {
    Ok(body) -> json_response(200, body)
    Error(error) -> actor_error_response(error)
  }
}

fn actor_error_response(error: String) -> response.Response(mist.ResponseData) {
  let status = case
    string.contains(error, "queue depth")
    || string.contains(error, "capacity reached")
    || string.contains(error, "concurrency wait")
  {
    True -> 429
    False ->
      case string.contains(error, "not found") {
        True -> 404
        False ->
          case
            string.contains(error, "unavailable")
            || string.contains(error, "LAMBDA_DATABASE_URL")
            || string.contains(error, "psql executable")
            || string.contains(error, "does not exist")
          {
            True -> 503
            False ->
              case
                string.contains(error, "valid")
                || string.contains(error, "unsupported")
                || string.contains(error, "requires")
              {
                True -> 400
                False -> 502
              }
          }
      }
  }
  json_response(
    status,
    "{\"ok\":false,\"error\":\"" <> json_escape(error) <> "\"}",
  )
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
                child_error_status(error),
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

fn invoke_release(
  req: request.Request(mist.Connection),
  function_id: String,
  qualifier: String,
) -> response.Response(mist.ResponseData) {
  let affinity = affinity_key(req)
  case mist.read_body(req, max_body_bytes) {
    Ok(req) -> {
      case bit_array.to_string(req.body) {
        Ok(payload) -> {
          case
            child_process.invoke_qualified(
              default_command,
              function_id,
              qualifier,
              affinity,
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
                child_error_status(error),
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

fn invoke_stream(
  req: request.Request(mist.Connection),
  function_id: String,
) -> response.Response(mist.ResponseData) {
  case mist.read_body(req, max_body_bytes) {
    Ok(body_req) ->
      case bit_array.to_string(body_req.body) {
        Ok(payload) -> {
          let initial_response =
            response.new(200)
            |> response.set_header("content-type", "application/octet-stream")
            |> response.set_header("cache-control", "no-store")
            |> response.set_header("x-content-type-options", "nosniff")
            |> response.set_header("x-scintilla-stream-protocol", "raw-v1")
          mist.chunked(
            request: req,
            response: initial_response,
            init: fn(subject) {
              let _ =
                child_process.start_stream(
                  default_command,
                  function_id,
                  request_payload(payload),
                  child_idle_ms,
                  300_000,
                  subject,
                )
              Nil
            },
            loop: stream_loop,
          )
        }
        Error(_) ->
          json_response(400, "{\"ok\":false,\"error\":\"body-not-utf8\"}")
      }
    Error(_) -> json_response(400, "{\"ok\":false,\"error\":\"invalid-body\"}")
  }
}

fn invoke_stream_release(
  req: request.Request(mist.Connection),
  function_id: String,
  qualifier: String,
) -> response.Response(mist.ResponseData) {
  let affinity = affinity_key(req)
  case mist.read_body(req, max_body_bytes) {
    Ok(body_req) ->
      case bit_array.to_string(body_req.body) {
        Ok(payload) -> {
          let initial_response =
            response.new(200)
            |> response.set_header("content-type", "application/octet-stream")
            |> response.set_header("cache-control", "no-store")
            |> response.set_header("x-content-type-options", "nosniff")
            |> response.set_header("x-scintilla-stream-protocol", "raw-v1")
          mist.chunked(
            request: req,
            response: initial_response,
            init: fn(subject) {
              let _ =
                child_process.start_stream_qualified(
                  default_command,
                  function_id,
                  qualifier,
                  affinity,
                  request_payload(payload),
                  child_idle_ms,
                  300_000,
                  subject,
                )
              Nil
            },
            loop: stream_loop,
          )
        }
        Error(_) ->
          json_response(400, "{\"ok\":false,\"error\":\"body-not-utf8\"}")
      }
    Error(_) -> json_response(400, "{\"ok\":false,\"error\":\"invalid-body\"}")
  }
}

fn stream_loop(
  state: Nil,
  message: child_process.StreamMessage,
  connection: mist.Connection,
) -> mist.ChunkNext(Nil) {
  case message {
    child_process.StreamChunk(data:, acknowledge:) -> {
      let sent = mist.send_chunk(connection, data)
      // Always release the runtime worker. A failed socket send stops the
      // connection actor immediately after the acknowledgement.
      process.send(acknowledge, Nil)
      case sent {
        Ok(_) -> mist.chunk_continue(state)
        Error(_) -> mist.chunk_stop_abnormal("stream client disconnected")
      }
    }
    child_process.StreamFinished -> mist.chunk_stop()
    child_process.StreamFailed(_) ->
      mist.chunk_stop_abnormal("stream invocation failed")
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
                child_error_status(error),
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

fn child_error_status(error: String) -> Int {
  case string.contains(error, "concurrency limit reached") {
    True -> 429
    False ->
      case string.contains(error, "not found") {
        True -> 404
        False ->
          case
            string.contains(error, "unavailable")
            || string.contains(error, "LAMBDA_DATABASE_URL")
            || string.contains(error, "psql executable")
            || string.contains(error, "does not exist")
          {
            True -> 503
            False ->
              case
                string.contains(error, "valid")
                || string.contains(error, "exceeds")
              {
                True -> 400
                False -> 502
              }
          }
      }
  }
}

fn async_invoke(
  req: request.Request(mist.Connection),
  function_id: String,
) -> response.Response(mist.ResponseData) {
  with_body(req, fn(payload) {
    workflow_result_response(
      202,
      "invocation",
      async_invocation.start(function_id, payload),
    )
  })
}

fn async_invoke_release(
  req: request.Request(mist.Connection),
  function_id: String,
  qualifier: String,
) -> response.Response(mist.ResponseData) {
  with_body(req, fn(payload) {
    workflow_result_response(
      202,
      "invocation",
      async_invocation.start_qualified(
        function_id,
        qualifier,
        affinity_key(req),
        payload,
      ),
    )
  })
}

fn async_get(run_id: String) -> response.Response(mist.ResponseData) {
  case async_invocation.get(run_id) {
    Ok(invocation) -> json_response(200, invocation)
    Error(error) -> workflow_error_response(error)
  }
}

fn async_cancel(run_id: String) -> response.Response(mist.ResponseData) {
  workflow_result_response(200, "invocation", async_invocation.cancel(run_id))
}

fn event_route(
  req: request.Request(mist.Connection),
) -> response.Response(mist.ResponseData) {
  with_body(req, fn(payload) {
    case events.route(payload) {
      Ok(summary) -> json_response(202, summary)
      Error(error) -> event_error_response(error)
    }
  })
}

fn event_error_response(error: String) -> response.Response(mist.ResponseData) {
  let status = case
    string.contains(error, "concurrency limit reached")
    || string.contains(error, "target limit exceeded")
  {
    True -> 429
    False ->
      case
        string.contains(error, "unavailable")
        || string.contains(error, "LAMBDA_DATABASE_URL")
        || string.contains(error, "psql executable")
      {
        True -> 503
        False ->
          case
            string.contains(error, "CloudEvent")
            || string.contains(error, "invalid")
          {
            True -> 400
            False -> 502
          }
      }
  }
  json_response(
    status,
    "{\"ok\":false,\"error\":\"" <> json_escape(error) <> "\"}",
  )
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
    Ok(runs) -> json_response(200, "{\"ok\":true,\"runs\":" <> runs <> "}")
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
      json_response(ok_status, "{\"ok\":true,\"" <> key <> "\":" <> body <> "}")
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
  case
    string.contains(error, "unavailable")
    || string.contains(error, "LAMBDA_DATABASE_URL")
    || string.contains(error, "psql executable")
  {
    True -> 503
    False ->
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

fn affinity_key(req: request.Request(mist.Connection)) -> String {
  case request.get_header(req, "x-scintilla-affinity") {
    Ok(value) -> value
    Error(_) -> query_value(req, "affinity")
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
      <> ",\"scheduleEngineEnabled\":"
      <> bool_json(schedule.enabled())
      <> ",\"eventRouterEnabled\":"
      <> bool_json(events.enabled())
      <> ",\"revisionRoutingEnabled\":"
      <> bool_json(revision_routing_enabled())
      <> ",\"actorEngineEnabled\":"
      <> bool_json(actors.enabled())
      <> ",\"actorSupervisorHealthy\":"
      <> bool_json(actors.healthy())
      <> ",\"runtimeSupervisorHealthy\":"
      <> bool_json(runtime_supervisor.healthy())
      <> "}",
  )
}

fn readyz() -> response.Response(mist.ResponseData) {
  let ready =
    runtime_supervisor.healthy()
    && actors.healthy()
    && !runtime_supervisor.draining()
  json_response(
    case ready {
      True -> 200
      False -> 503
    },
    "{\"ok\":"
      <> bool_json(ready)
      <> ",\"runtimeSupervisorHealthy\":"
      <> bool_json(runtime_supervisor.healthy())
      <> ",\"actorSupervisorHealthy\":"
      <> bool_json(actors.healthy())
      <> ",\"draining\":"
      <> bool_json(runtime_supervisor.draining())
      <> "}",
  )
}

fn runtime_processes() -> response.Response(mist.ResponseData) {
  json_response(
    200,
    "{\"ok\":true,\"runtimes\":"
      <> runtime_supervisor.snapshot()
      <> ",\"actors\":"
      <> actors.snapshot()
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
      child_process.metrics()
      <> "\n"
      <> runtime_supervisor.metrics()
      <> "\n"
      <> workflow.metrics()
      <> "\n"
      <> schedule.metrics()
      <> "\n"
      <> events.metrics()
      <> "\n"
      <> actors.metrics(),
    )),
  )
}

fn not_found() -> response.Response(mist.ResponseData) {
  json_response(404, "{\"ok\":false,\"error\":\"not-found\"}")
}

fn method_not_allowed() -> response.Response(mist.ResponseData) {
  response.new(405)
  |> response.set_header("allow", "GET, POST, DELETE")
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

fn revision_routing_enabled() -> Bool {
  env_get("LAMBDA_REVISION_ROUTING_ENABLED") == "1"
  || env_get("LAMBDA_REVISION_ROUTING_ENABLED") == "true"
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

const home_html = "<!doctype html><html><head><meta charset=\"utf-8\"/><title>dd gleam lambda runner</title><style>body{font-family:system-ui;margin:24px;line-height:1.45}code{background:#f1f5f9;padding:2px 5px;border-radius:4px}pre{max-height:50vh;overflow:auto;background:#111827;color:#d1fae5;padding:12px;border-radius:8px}</style></head><body><h1>dd gleam lambda runner</h1><p>Liveness: <code>/healthz</code> · readiness/draining: <code>/readyz</code></p><p>Metrics: <code>/metrics</code></p><p>Invocation endpoint: <code>POST /invoke/:function_id</code>. Gateway invocation traffic lands here directly; the child runner loads the active function definition from Postgres and Gleam manages reusable child processes under an OTP supervision tree.</p><p>Immutable releases: <code>POST /invoke/:function_id/revisions/:revision</code> and <code>POST /invoke/:function_id/aliases/:alias</code>. Weighted aliases can use <code>X-Scintilla-Affinity</code> for sticky canaries without restarting the BEAM server.</p><p>Durable actors: <code>POST /actors/:function_id/:actor_key</code>. Each hot key is a dynamically supervised BEAM process with Postgres-fenced state and alarms.</p></body></html>"
