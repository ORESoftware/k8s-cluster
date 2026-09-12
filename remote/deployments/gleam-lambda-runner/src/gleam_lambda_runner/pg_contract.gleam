import pg_defs

pub fn lambda_functions_select_sql() -> String {
  pg_defs.lambda_functions_select_sql
}

pub fn workflow_definitions_select_sql() -> String {
  pg_defs.workflow_definitions_select_sql
}

pub fn workflow_runs_select_sql() -> String {
  pg_defs.workflow_runs_select_sql
}

pub fn workflow_step_runs_select_sql() -> String {
  pg_defs.workflow_step_runs_select_sql
}
