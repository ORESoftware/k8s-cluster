//// Tests for the workflow-engine FFI boundary and request validation. These run
//// without a database: they exercise the Gleam -> Erlang wiring (function names,
//// arities, and the {ok,_}/{error,_} -> Result mapping) and the validation
//// branches that fail fast before any psql call.

import gleam/string
import gleam_lambda_runner/workflow
import gleeunit/should

@external(erlang, "lambda_runtime_env", "putenv")
fn putenv(name: String, value: String) -> Bool

fn without_database() -> Nil {
  let _ = putenv("LAMBDA_DATABASE_URL", "")
  let _ = putenv("WORKFLOW_ENGINE_ENABLED", "")
  Nil
}

pub fn engine_disabled_without_database_test() {
  without_database()
  workflow.enabled()
  |> should.be_false
}

pub fn metrics_exposes_workflow_counters_test() {
  workflow.metrics()
  |> string.contains("workflow_runs_started_total")
  |> should.be_true
}

pub fn start_run_requires_body_test() {
  without_database()
  workflow.start_run("")
  |> should.be_error
}

pub fn start_run_requires_definition_ref_test() {
  without_database()
  let result = workflow.start_run("{\"input\":{}}")
  should.be_error(result)
  case result {
    Error(message) ->
      string.contains(message, "definitionId")
      |> should.be_true
    Ok(_) -> should.fail()
  }
}

pub fn start_run_rejects_invalid_json_test() {
  without_database()
  let result = workflow.start_run("{not json")
  should.be_error(result)
  case result {
    Error(message) ->
      string.contains(message, "JSON")
      |> should.be_true
    Ok(_) -> should.fail()
  }
}

pub fn signal_requires_name_test() {
  without_database()
  let result =
    workflow.signal_run("11111111-1111-1111-1111-111111111111", "{}")
  should.be_error(result)
  case result {
    Error(message) ->
      string.contains(message, "name")
      |> should.be_true
    Ok(_) -> should.fail()
  }
}

pub fn cancel_validates_run_uuid_test() {
  without_database()
  let result = workflow.cancel_run("not-a-uuid")
  should.be_error(result)
  case result {
    Error(message) ->
      string.contains(message, "UUID")
      |> should.be_true
    Ok(_) -> should.fail()
  }
}

pub fn get_run_validates_run_uuid_test() {
  without_database()
  let result = workflow.get_run("not-a-uuid")
  should.be_error(result)
  case result {
    Error(message) ->
      string.contains(message, "UUID")
      |> should.be_true
    Ok(_) -> should.fail()
  }
}

pub fn list_runs_without_database_errors_test() {
  without_database()
  workflow.list_runs("", 10)
  |> should.be_error
}
