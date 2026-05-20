//// PG contract surface for `gleamlang-ws-server`.
//// Re-exports the shared `dd_pg_defs` schema constants so the merged
//// WebSocket service has a single import site when it needs to read
//// shared app config from RDS Postgres. Adding a usage here also
//// guarantees the `dd_pg_defs` path dep is exercised at build time.
//// See `remote/libs/pg-defs/readme.md` for the source-of-truth contract.

import pg_defs

/// Canonical SELECT for the shared `app_config` table.
pub fn app_config_select_sql() -> String {
  pg_defs.app_config_select_sql
}

pub fn app_config_table() -> String {
  pg_defs.app_config_table
}
