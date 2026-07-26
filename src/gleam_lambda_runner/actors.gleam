//// Dynamically supervised, Postgres-durable keyed actors.
////
//// Each hot actor is an Erlang process with a single ordered mailbox. The
//// process may disappear after idling because state, version fencing,
//// cross-replica leases, and alarms survive in Postgres.

import gleam/dynamic
import gleam/erlang/process
import gleam/otp/actor
import gleam/otp/supervision

const default_command = "env -i PATH=\"$PATH\" NODE_ENV=production NODE_NO_WARNINGS=1 NATS_URL=\"${NATS_URL:-}\" CONTAINER_POOL_NATS_URL=\"${CONTAINER_POOL_NATS_URL:-}\" CONTAINER_POOL_NATS_SUBJECT_PREFIX=\"${CONTAINER_POOL_NATS_SUBJECT_PREFIX:-dd.remote.container_pool}\" CONTAINER_POOL_NATS_TIMEOUT_MS=\"${CONTAINER_POOL_NATS_TIMEOUT_MS:-30000}\" LAMBDA_STREAM_MAX_BYTES=\"${LAMBDA_STREAM_MAX_BYTES:-16777216}\" LAMBDA_STREAM_CHUNK_BYTES=\"${LAMBDA_STREAM_CHUNK_BYTES:-65536}\" LAMBDA_ACTOR_STATE_MAX_BYTES=\"${LAMBDA_ACTOR_STATE_MAX_BYTES:-524288}\" child-runtimes/node-permission-launcher.sh --allow-fs-read=child-runtimes --allow-fs-read=../../../../libs child-runtimes/js-function-runner.mjs"

@external(erlang, "lambda_actor_supervisor", "start_link")
fn start_erlang_supervisor(
  command: String,
) -> Result(process.Pid, dynamic.Dynamic)

@external(erlang, "lambda_actor_manager", "invoke")
fn invoke_erlang(
  command: String,
  function_ref: String,
  actor_key: String,
  payload: String,
  idle_ms: Int,
  timeout_ms: Int,
) -> Result(String, String)

@external(erlang, "lambda_actor_manager", "reset")
pub fn reset(function_ref: String, actor_key: String) -> Result(String, String)

@external(erlang, "lambda_actor_manager", "get_state")
pub fn get_state(
  function_ref: String,
  actor_key: String,
) -> Result(String, String)

@external(erlang, "lambda_actor_manager", "enabled")
pub fn enabled() -> Bool

@external(erlang, "lambda_actor_manager", "snapshot")
pub fn snapshot() -> String

@external(erlang, "lambda_actor_manager", "metrics")
pub fn metrics() -> String

@external(erlang, "lambda_actor_supervisor", "healthy")
pub fn healthy() -> Bool

pub fn invoke(
  function_ref: String,
  actor_key: String,
  payload: String,
) -> Result(String, String) {
  invoke_erlang(
    default_command,
    function_ref,
    actor_key,
    payload,
    60_000,
    300_000,
  )
}

fn start() -> actor.StartResult(Nil) {
  case start_erlang_supervisor(default_command) {
    Ok(pid) -> Ok(actor.Started(pid: pid, data: Nil))
    Error(_) ->
      Error(actor.InitFailed("could not start the durable actor supervisor"))
  }
}

pub fn supervised() {
  supervision.supervisor(start)
}
