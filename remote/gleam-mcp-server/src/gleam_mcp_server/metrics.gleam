import gleam/erlang/process
import gleam/otp/actor

pub type Snapshot {
  Snapshot(
    http_requests: Int,
    rpc_requests: Int,
    initialize_requests: Int,
    tools_list_requests: Int,
    tools_call_requests: Int,
    ping_requests: Int,
    unknown_requests: Int,
  )
}

pub type Message {
  RecordHttpRequest
  RecordRpcRequest(String)
  GetSnapshot(process.Subject(Snapshot))
}

type State {
  State(
    http_requests: Int,
    rpc_requests: Int,
    initialize_requests: Int,
    tools_list_requests: Int,
    tools_call_requests: Int,
    ping_requests: Int,
    unknown_requests: Int,
  )
}

pub fn start(
  named_as name: process.Name(Message),
) -> actor.StartResult(process.Subject(Message)) {
  actor.new(State(
    http_requests: 0,
    rpc_requests: 0,
    initialize_requests: 0,
    tools_list_requests: 0,
    tools_call_requests: 0,
    ping_requests: 0,
    unknown_requests: 0,
  ))
  |> actor.named(name)
  |> actor.on_message(handle_message)
  |> actor.start
}

fn handle_message(state: State, message: Message) -> actor.Next(State, Message) {
  let State(
    http_requests: http_requests,
    rpc_requests: rpc_requests,
    initialize_requests: initialize_requests,
    tools_list_requests: tools_list_requests,
    tools_call_requests: tools_call_requests,
    ping_requests: ping_requests,
    unknown_requests: unknown_requests,
  ) = state

  case message {
    RecordHttpRequest ->
      actor.continue(State(
        http_requests: http_requests + 1,
        rpc_requests: rpc_requests,
        initialize_requests: initialize_requests,
        tools_list_requests: tools_list_requests,
        tools_call_requests: tools_call_requests,
        ping_requests: ping_requests,
        unknown_requests: unknown_requests,
      ))

    RecordRpcRequest(method) -> {
      let is_initialize = method == "initialize"
      let is_tools_list = method == "tools/list"
      let is_tools_call = method == "tools/call"
      let is_ping = method == "ping"

      actor.continue(State(
        http_requests: http_requests,
        rpc_requests: rpc_requests + 1,
        initialize_requests: initialize_requests + bool_to_int(is_initialize),
        tools_list_requests: tools_list_requests + bool_to_int(is_tools_list),
        tools_call_requests: tools_call_requests + bool_to_int(is_tools_call),
        ping_requests: ping_requests + bool_to_int(is_ping),
        unknown_requests: unknown_requests
          + bool_to_int(!is_initialize && !is_tools_list && !is_tools_call && !is_ping),
      ))
    }

    GetSnapshot(reply_to) -> {
      process.send(reply_to, Snapshot(
        http_requests: http_requests,
        rpc_requests: rpc_requests,
        initialize_requests: initialize_requests,
        tools_list_requests: tools_list_requests,
        tools_call_requests: tools_call_requests,
        ping_requests: ping_requests,
        unknown_requests: unknown_requests,
      ))
      actor.continue(state)
    }
  }
}

fn bool_to_int(value: Bool) -> Int {
  case value {
    True -> 1
    False -> 0
  }
}
