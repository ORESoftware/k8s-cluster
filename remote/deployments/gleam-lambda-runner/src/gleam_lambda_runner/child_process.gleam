@external(erlang, "lambda_child_runner", "invoke")
pub fn invoke(
  command: String,
  reuse_key: String,
  payload: String,
  idle_ms: Int,
  timeout_ms: Int,
) -> Result(String, String)

@external(erlang, "lambda_child_runner", "invoke_definition")
pub fn invoke_definition(
  command: String,
  reuse_key: String,
  definition: String,
  payload: String,
  idle_ms: Int,
  timeout_ms: Int,
) -> Result(String, String)

@external(erlang, "lambda_child_runner", "metrics")
pub fn metrics() -> String

@external(erlang, "lambda_child_runner", "destroy")
pub fn destroy(reuse_key: String) -> Result(String, String)
