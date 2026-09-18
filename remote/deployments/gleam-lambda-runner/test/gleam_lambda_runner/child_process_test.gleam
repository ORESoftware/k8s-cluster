import gleam/string
import gleam_lambda_runner/child_process
import gleeunit/should

@external(erlang, "lambda_runtime_env", "putenv")
fn putenv(name: String, value: String) -> Bool

const local_node_command = "env -i PATH=\"$PATH\" NODE_ENV=production node child-runtimes/js-function-runner.mjs"

const reuse_definition = "{"
  <> "\"id\":\"00000000-0000-0000-0000-000000000001\","
  <> "\"slug\":\"reuse-check\","
  <> "\"functionBody\":\"globalThis.__ddReuseCount = (globalThis.__ddReuseCount || 0) + 1; return { status: 200, body: { count: globalThis.__ddReuseCount, echo: request.body } };\","
  <> "\"runtime\":\"nodejs\","
  <> "\"status\":\"active\","
  <> "\"containerized\":false,"
  <> "\"reuseKey\":\"reuse-test\""
  <> "}"

pub fn child_runner_reuses_host_worker_test() {
  let _ = putenv("LAMBDA_ALLOW_HOST_RUNTIMES", "nodejs")
  let _ = putenv("LAMBDA_PREWARM_RUNTIMES", "none")
  let _ = putenv("LAMBDA_NODEJS_HOST_COMMAND", local_node_command)
  let _ = child_process.destroy("function:reuse-check:reuse-test")

  let assert Ok(first) =
    child_process.invoke_definition(
      local_node_command,
      "reuse-check",
      reuse_definition,
      "{\"body\":{\"n\":1}}",
      300_000,
      30_000,
    )
  let assert Ok(second) =
    child_process.invoke_definition(
      local_node_command,
      "reuse-check",
      reuse_definition,
      "{\"body\":{\"n\":2}}",
      300_000,
      30_000,
    )

  should.be_true(string.contains(first, "\"count\":1"))
  should.be_true(string.contains(second, "\"count\":2"))
  child_process.metrics()
  |> string.contains("dd_lambda_runner_child_reuses_total")
  |> should.be_true

  let _ = child_process.destroy("function:reuse-check:reuse-test")
}
