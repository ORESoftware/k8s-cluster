//// Supervised CloudEvents 1.0 router.

import gleam/dynamic
import gleam/erlang/process
import gleam/otp/actor
import gleam/otp/supervision

@external(erlang, "lambda_events", "start_link")
fn start_erlang_router() -> Result(process.Pid, dynamic.Dynamic)

@external(erlang, "lambda_events", "enabled")
pub fn enabled() -> Bool

@external(erlang, "lambda_events", "route_from_body")
pub fn route(body: String) -> Result(String, String)

@external(erlang, "lambda_events", "metrics")
pub fn metrics() -> String

fn start() -> actor.StartResult(Nil) {
  case start_erlang_router() {
    Ok(pid) -> Ok(actor.Started(pid: pid, data: Nil))
    Error(_) ->
      Error(actor.InitFailed("could not start the CloudEvents router"))
  }
}

pub fn supervised() {
  supervision.worker(start)
}
