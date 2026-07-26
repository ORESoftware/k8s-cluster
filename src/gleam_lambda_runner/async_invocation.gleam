//// Durable asynchronous invocation backed by the supervised workflow engine.
//// The Erlang module owns validation and persistence; this module keeps the
//// HTTP surface typed and small.

@external(erlang, "lambda_async", "start_from_body")
pub fn start(function_ref: String, body: String) -> Result(String, String)

@external(erlang, "lambda_async", "start_qualified_from_body")
pub fn start_qualified(
  function_ref: String,
  qualifier: String,
  affinity: String,
  body: String,
) -> Result(String, String)

@external(erlang, "lambda_async", "get")
pub fn get(run_id: String) -> Result(String, String)

@external(erlang, "lambda_async", "cancel")
pub fn cancel(run_id: String) -> Result(String, String)
