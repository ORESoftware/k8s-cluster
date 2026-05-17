//// Smoke tests for the generated `dd_pg_defs` package.
//// These run via `gleam test` and prove that the canonical table constants
//// and SELECT SQL strings are non-empty + name the right table. Any future
//// regenerator change that breaks the contract is caught here before a
//// downstream consumer (lambda runner / mcp / gleamlang server) silently
//// picks up a broken adapter.

import gleam/string
import gleeunit
import gleeunit/should
import pg_defs

pub fn main() {
  gleeunit.main()
}

pub fn every_canonical_table_is_named_test() {
  // Every shared table in schema.sql must have a `_table` constant exposed
  // here. If you add a table to schema.sql, regenerate the bindings and add
  // it to this list so the wiring is locked in.
  let tables = [
    pg_defs.app_config_table,
    pg_defs.container_pool_configs_table,
    pg_defs.known_git_repos_table,
    pg_defs.agent_remote_dev_threads_table,
    pg_defs.agent_remote_dev_tasks_table,
    pg_defs.agent_remote_dev_events_table,
    pg_defs.agent_remote_dev_artifacts_table,
    pg_defs.agent_remote_dev_runtime_locks_table,
    pg_defs.lambda_functions_table,
  ]
  should.equal(9, list_length(tables))
  each_is_non_empty(tables)
}

pub fn each_table_name_is_distinct_test() {
  let tables = [
    pg_defs.app_config_table,
    pg_defs.container_pool_configs_table,
    pg_defs.known_git_repos_table,
    pg_defs.agent_remote_dev_threads_table,
    pg_defs.agent_remote_dev_tasks_table,
    pg_defs.agent_remote_dev_events_table,
    pg_defs.agent_remote_dev_artifacts_table,
    pg_defs.agent_remote_dev_runtime_locks_table,
    pg_defs.lambda_functions_table,
  ]
  should.equal(9, list_length(tables))
  let deduped = dedupe(tables)
  should.equal(9, list_length(deduped))
}

pub fn app_config_select_targets_the_right_table_test() {
  let sql = pg_defs.app_config_select_sql
  should.be_true(string.contains(sql, "from app_config"))
  should.be_true(string.contains(sql, "value::text as value_json"))
  should.be_true(string.contains(sql, "is_soft_deleted"))
}

pub fn lambda_functions_select_targets_the_right_table_test() {
  let sql = pg_defs.lambda_functions_select_sql
  should.be_true(string.contains(sql, "from lambda_functions"))
  // The container build status enum lives on this row; if it ever falls
  // off the SELECT the lambda runner can't reason about build state.
  should.be_true(string.contains(sql, "container_build_status"))
}

pub fn app_config_status_round_trips_test() {
  pg_defs.app_config_status_to_string(pg_defs.AppConfigStatusActive)
  |> should.equal("active")
  pg_defs.app_config_status_to_string(pg_defs.AppConfigStatusPaused)
  |> should.equal("paused")
  pg_defs.app_config_status_to_string(pg_defs.AppConfigStatusArchived)
  |> should.equal("archived")

  pg_defs.parse_app_config_status("active")
  |> should.equal(Ok(pg_defs.AppConfigStatusActive))
  pg_defs.parse_app_config_status("paused")
  |> should.equal(Ok(pg_defs.AppConfigStatusPaused))
  pg_defs.parse_app_config_status("archived")
  |> should.equal(Ok(pg_defs.AppConfigStatusArchived))
  // Bogus inputs must fail loudly; silently coercing to a default would
  // hide schema drift between the canonical SQL enum and this adapter.
  let result = pg_defs.parse_app_config_status("nonsense")
  case result {
    Error(_) -> Nil
    Ok(_) -> should.fail()
  }
}

fn each_is_non_empty(values: List(String)) -> Nil {
  case values {
    [] -> Nil
    [head, ..rest] -> {
      should.be_true(string.length(head) > 0)
      each_is_non_empty(rest)
    }
  }
}

fn list_length(values: List(a)) -> Int {
  list_length_loop(values, 0)
}

fn list_length_loop(values: List(a), acc: Int) -> Int {
  case values {
    [] -> acc
    [_, ..rest] -> list_length_loop(rest, acc + 1)
  }
}

fn dedupe(values: List(String)) -> List(String) {
  dedupe_loop(values, [])
}

fn dedupe_loop(values: List(String), acc: List(String)) -> List(String) {
  case values {
    [] -> acc
    [head, ..rest] -> {
      case contains(acc, head) {
        True -> dedupe_loop(rest, acc)
        False -> dedupe_loop(rest, [head, ..acc])
      }
    }
  }
}

fn contains(values: List(String), needle: String) -> Bool {
  case values {
    [] -> False
    [head, ..rest] ->
      case head == needle {
        True -> True
        False -> contains(rest, needle)
      }
  }
}
