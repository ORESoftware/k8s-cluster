//// Proves the `dd_pg_defs` path-dep is actually wired into
//// `dd-gleam-mcp-server`. Whenever MCP tooling reads from RDS it should go
//// through this re-export so the schema source-of-truth stays single, and
//// this test catches drift between the local re-export and the canonical
//// constants in `dd_pg_defs`.

import gleam/string
import gleam_mcp_server/pg_contract
import gleeunit/should
import pg_defs

pub fn app_config_table_matches_canonical_test() {
  pg_contract.app_config_table()
  |> should.equal(pg_defs.app_config_table)
  pg_contract.app_config_table()
  |> should.equal("app_config")
}

pub fn app_config_select_sql_matches_canonical_test() {
  pg_contract.app_config_select_sql()
  |> should.equal(pg_defs.app_config_select_sql)
}

pub fn lambda_functions_select_sql_matches_canonical_test() {
  pg_contract.lambda_functions_select_sql()
  |> should.equal(pg_defs.lambda_functions_select_sql)
}

pub fn select_sql_targets_real_tables_test() {
  let app_sql = pg_contract.app_config_select_sql()
  should.be_true(string.contains(app_sql, "from app_config"))
  let lambda_sql = pg_contract.lambda_functions_select_sql()
  should.be_true(string.contains(lambda_sql, "from lambda_functions"))
}
