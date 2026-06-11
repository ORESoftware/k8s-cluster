//// App-facing interface to the workflow execution engine. The raw scheduler,
//// durable step-state machine, and Postgres persistence live in the Erlang
//// modules `workflow_engine` and `workflow_store` inside the same BEAM VM, the
//// same split used by `nats.gleam` / `lambda_nats.erl`.
////
//// The engine functions return `{ok, Json} | {error, Json}` in Erlang, which is
//// exactly Gleam's `Result(String, String)` at runtime, so the HTTP layer can
//// bind them directly. Request bodies are parsed in Erlang (OTP `json`).

@external(erlang, "workflow_engine", "start_for_gleam")
fn engine_start() -> Nil

@external(erlang, "workflow_engine", "enabled")
pub fn enabled() -> Bool

@external(erlang, "workflow_engine", "start_run_from_body")
pub fn start_run(body: String) -> Result(String, String)

@external(erlang, "workflow_engine", "get_run")
pub fn get_run(run_id: String) -> Result(String, String)

@external(erlang, "workflow_engine", "list_runs")
pub fn list_runs(definition_ref: String, limit: Int) -> Result(String, String)

@external(erlang, "workflow_engine", "signal_from_body")
pub fn signal_run(run_id: String, body: String) -> Result(String, String)

@external(erlang, "workflow_engine", "cancel_run")
pub fn cancel_run(run_id: String) -> Result(String, String)

@external(erlang, "workflow_engine", "metrics")
pub fn metrics() -> String

/// Start the engine as a detached singleton (mirrors `nats.start`). Safe to call
/// when no database is configured: the engine stays disabled and idle.
pub fn start() -> Nil {
  engine_start()
}
