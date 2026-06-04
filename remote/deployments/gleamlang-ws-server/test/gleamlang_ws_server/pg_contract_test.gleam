//// Proves the `dd_pg_defs` path-dep is actually wired into
//// `dd-gleamlang-ws-server`. Reads against `app_config` from this server
//// must always go through the canonical re-export so the schema source
//// of truth stays single, and this test catches drift before it ships.

import gleam/string
import gleamlang_ws_server/pg_contract
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

pub fn select_sql_targets_app_config_table_test() {
  let sql = pg_contract.app_config_select_sql()
  should.be_true(string.contains(sql, "from app_config"))
  should.be_true(string.contains(sql, "is_soft_deleted"))
}
