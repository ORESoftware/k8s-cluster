//// Supervised UTC cron-to-CloudEvents scheduler.

import gleam/dynamic
import gleam/erlang/process
import gleam/otp/actor
import gleam/otp/supervision

@external(erlang, "lambda_schedule", "start_link")
fn start_erlang_scheduler() -> Result(process.Pid, dynamic.Dynamic)

@external(erlang, "lambda_schedule", "enabled")
pub fn enabled() -> Bool

@external(erlang, "lambda_schedule", "metrics")
pub fn metrics() -> String

fn start() -> actor.StartResult(Nil) {
  case start_erlang_scheduler() {
    Ok(pid) -> Ok(actor.Started(pid: pid, data: Nil))
    Error(_) ->
      Error(actor.InitFailed("could not start the lambda schedule engine"))
  }
}

pub fn supervised() {
  supervision.worker(start)
}
