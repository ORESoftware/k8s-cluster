import gleam/erlang/process

pub type StreamMessage {
  StreamChunk(data: BitArray, acknowledge: process.Subject(Nil))
  StreamFinished
  StreamFailed(reason: String)
}

@external(erlang, "lambda_child_runner", "invoke")
pub fn invoke(
  command: String,
  reuse_key: String,
  payload: String,
  idle_ms: Int,
  timeout_ms: Int,
) -> Result(String, String)

@external(erlang, "lambda_child_runner", "invoke_qualified")
pub fn invoke_qualified(
  command: String,
  function_id: String,
  qualifier: String,
  affinity: String,
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

@external(erlang, "lambda_child_runner", "invoke_stream")
fn invoke_stream_ffi(
  command: String,
  function_id: String,
  payload: String,
  idle_ms: Int,
  timeout_ms: Int,
  emit: fn(BitArray) -> Nil,
) -> Result(Int, String)

@external(erlang, "lambda_child_runner", "invoke_stream_qualified")
fn invoke_stream_qualified_ffi(
  command: String,
  function_id: String,
  qualifier: String,
  affinity: String,
  payload: String,
  idle_ms: Int,
  timeout_ms: Int,
  emit: fn(BitArray) -> Nil,
) -> Result(Int, String)

/// Start a linked streaming invocation. Every callback synchronously calls the
/// connection actor and waits for its socket-send acknowledgement, carrying
/// network backpressure through the BEAM port to the child runtime.
pub fn start_stream(
  command: String,
  function_id: String,
  payload: String,
  idle_ms: Int,
  timeout_ms: Int,
  target: process.Subject(StreamMessage),
) -> process.Pid {
  process.spawn(fn() {
    let result =
      invoke_stream_ffi(
        command,
        function_id,
        payload,
        idle_ms,
        timeout_ms,
        fn(data) {
          process.call_forever(target, fn(acknowledge) {
            StreamChunk(data:, acknowledge:)
          })
        },
      )
    case result {
      Ok(_) -> process.send(target, StreamFinished)
      Error(reason) -> process.send(target, StreamFailed(reason:))
    }
  })
}

/// Start a linked streaming invocation pinned through an immutable revision or
/// weighted alias. An explicit affinity value makes canary selection sticky.
pub fn start_stream_qualified(
  command: String,
  function_id: String,
  qualifier: String,
  affinity: String,
  payload: String,
  idle_ms: Int,
  timeout_ms: Int,
  target: process.Subject(StreamMessage),
) -> process.Pid {
  process.spawn(fn() {
    let result =
      invoke_stream_qualified_ffi(
        command,
        function_id,
        qualifier,
        affinity,
        payload,
        idle_ms,
        timeout_ms,
        fn(data) {
          process.call_forever(target, fn(acknowledge) {
            StreamChunk(data:, acknowledge:)
          })
        },
      )
    case result {
      Ok(_) -> process.send(target, StreamFinished)
      Error(reason) -> process.send(target, StreamFailed(reason:))
    }
  })
}

@external(erlang, "lambda_child_runner", "check_definition")
pub fn check_definition(
  command: String,
  definition: String,
  timeout_ms: Int,
) -> Result(String, String)

@external(erlang, "lambda_child_runner", "metrics")
pub fn metrics() -> String

@external(erlang, "lambda_child_runner", "destroy")
pub fn destroy(reuse_key: String) -> Result(String, String)

@external(erlang, "lambda_child_runner", "container_command_for_test")
pub fn container_command_for_test(
  runtime: String,
  definition: String,
) -> Result(String, String)
