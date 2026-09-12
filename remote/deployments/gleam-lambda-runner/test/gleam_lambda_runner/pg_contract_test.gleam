//// Proves the `dd_pg_defs` path-dep is actually wired into
//// `dd-gleam-lambda-runner`: if the re-exported SQL drifts away from the
//// canonical schema (or the path-dep is silently dropped from gleam.toml),
//// this test catches it before the runner ships a stale read of
//// `lambda_functions`.

import gleam/string
import gleam_lambda_runner/pg_contract
import gleeunit/should
import pg_defs

pub fn lambda_functions_select_sql_matches_canonical_test() {
  pg_contract.lambda_functions_select_sql()
  |> should.equal(pg_defs.lambda_functions_select_sql)
}

pub fn lambda_functions_select_targets_the_right_table_test() {
  let sql = pg_contract.lambda_functions_select_sql()
  should.be_true(string.contains(sql, "from lambda_functions"))
  // The lambda runner branches on container build status; if the SELECT
  // drops this column the runner crashes at row-decode time.
  should.be_true(string.contains(sql, "container_build_status"))
}
