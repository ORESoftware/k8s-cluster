//// PG contract surface for dd-gleam-mcp-server.
//// Re-exports the shared `dd_pg_defs` schema constants so the MCP server has a
//// single import site when it wants to read from RDS Postgres via `psql` or a
//// future BEAM driver. Adding a usage here also guarantees the `dd_pg_defs`
//// path dep is exercised at build time.
//// See `remote/libs/pg-defs/readme.md` for the source-of-truth contract.

import pg_defs

/// Canonical SELECT for the shared `app_config` table.
/// MCP tools should use this rather than hand-written SQL so that schema
/// changes are picked up via the generated `dd_pg_defs` package.
pub fn app_config_select_sql() -> String {
  pg_defs.app_config_select_sql
}

pub fn app_config_table() -> String {
  pg_defs.app_config_table
}

/// Canonical SELECT for the lambda-functions registry. The MCP server exposes
/// read-only cluster tools today; pulling this constant in keeps the contract
/// aligned with `dd-gleam-lambda-runner` and the `dd-remote-rest-api`.
pub fn lambda_functions_select_sql() -> String {
  pg_defs.lambda_functions_select_sql
}
