import gleam/dynamic
import gleam/erlang/process
import gleam/otp/actor
import gleam/otp/supervision

@external(erlang, "lambda_runtime_supervisor", "start_link")
fn start_erlang_supervisor() -> Result(process.Pid, dynamic.Dynamic)

@external(erlang, "lambda_runtime_supervisor", "healthy")
pub fn healthy() -> Bool

@external(erlang, "lambda_runtime_supervisor", "draining")
pub fn draining() -> Bool

@external(erlang, "lambda_runtime_supervisor", "snapshot")
pub fn snapshot() -> String

@external(erlang, "lambda_runtime_supervisor", "metrics")
pub fn metrics() -> String

fn start() -> actor.StartResult(Nil) {
  case start_erlang_supervisor() {
    Ok(pid) -> Ok(actor.Started(pid: pid, data: Nil))
    Error(_) ->
      Error(actor.InitFailed("could not start the BEAM runtime supervisor"))
  }
}

pub fn supervised() {
  supervision.supervisor(start)
}
