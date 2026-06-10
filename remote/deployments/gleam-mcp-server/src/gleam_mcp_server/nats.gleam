//// NATS transport for the MCP server.
////
//// The MCP server is an HTTP JSON-RPC service, but the rest of the dd
//// runtime is wired together over NATS. This module gives the server a
//// presence on the bus using the source-of-truth subject names from
//// `remote/libs/nats/subject-defs` (no magic strings):
////
////   PUBLISH dd.remote.mcp.tool.events  (McpToolEvents)
////     - one `dd.mcp_event.v1` lifecycle event shortly after boot
////     - one audit event per `tools/call` (tool name, ok flag, request id)
////
////   SUBSCRIBE dd.remote.mcp.control     (McpControl)
////     - read-only operational commands; `{"command":"ping"}` echoes a
////       `pong` event back onto McpToolEvents. Commands never touch the
////       cluster — the MCP service account is list/read only.
////
//// The transport is OPTIONAL: it only starts when `NATS_URL` is set, so
//// the HTTP surface is completely unchanged when NATS isn't configured.
//// Every PUB carries a `Source-Node` header set to this BEAM node so we
//// drop our own echoes when subscribed to a `both`-direction subject.
////
//// The underlying `dd_nats` Erlang gen_server reconnects with jittered
//// backoff and replays in-flight subscriptions, so callers never re-subscribe.

import dd_nats_subject_defs.{mcp_control_subject, mcp_tool_events_subject}
import gleam/bit_array
import gleam/dynamic.{type Dynamic}
import gleam/dynamic/decode
import gleam/erlang/atom
import gleam/erlang/process.{type Pid}
import gleam/int
import gleam/io
import gleam/list
import gleam/otp/actor
import gleam/otp/supervision
import gleam/result
import gleam/string

pub type Name =
  process.Name(Message)

pub opaque type Message {
  Publish(subject: String, payload: String)
  EmitLifecycle
  Inbound(subject: String, payload: BitArray, headers: List(#(String, String)))
}

type State {
  State(client_pid: Pid, self_node: String)
}

// ── FFI ──────────────────────────────────────────────────────────────────

@external(erlang, "dd_nats", "start_link")
fn dd_nats_start_link(url: String, notify: Pid) -> Result(Pid, Dynamic)

@external(erlang, "dd_nats", "publish")
fn dd_nats_publish(
  pid: Pid,
  subject: BitArray,
  payload: BitArray,
  headers: List(#(BitArray, BitArray)),
) -> Dynamic

@external(erlang, "dd_nats", "subscribe")
fn dd_nats_subscribe(pid: Pid, subject: BitArray) -> Result(Int, Dynamic)

@external(erlang, "gleam_mcp_nats", "self_node_binary")
fn self_node_binary() -> String

@external(erlang, "gleam_mcp_nats", "now_ms")
fn now_ms() -> Int

// ── Lifecycle ─────────────────────────────────────────────────────────────

pub fn supervised(
  name name: Name,
  url url: String,
) -> supervision.ChildSpecification(process.Subject(Message)) {
  supervision.worker(fn() { start(name, url) })
}

pub fn start(
  name: Name,
  url: String,
) -> Result(actor.Started(process.Subject(Message)), actor.StartError) {
  actor.new_with_initialiser(2000, fn(self) {
    let self_pid = process.self()
    case dd_nats_start_link(url, self_pid) {
      Error(e) -> Error("dd_nats: " <> string.inspect(e))
      Ok(client) -> {
        // Listen for operational commands.
        let _ =
          dd_nats_subscribe(client, bit_array.from_string(mcp_control_subject))
        // Emit the boot lifecycle event once the TCP connect + CONNECT
        // handshake has had time to complete (publishing immediately would
        // race the socket and be dropped).
        process.send_after(self, 2000, EmitLifecycle)
        let selector =
          process.new_selector()
          |> process.select(self)
          |> process.select_record(
            tag: atom.create("nats_msg"),
            fields: 3,
            mapping: decode_nats_msg,
          )
        actor.initialised(State(
          client_pid: client,
          self_node: self_node_binary(),
        ))
        |> actor.selecting(selector)
        |> actor.returning(self)
        |> Ok
      }
    }
  })
  |> actor.named(name)
  |> actor.on_message(handle)
  |> actor.start()
}

// ── Public API (called from the HTTP layer) ────────────────────────────────

/// Publish a `tools/call` audit event onto McpToolEvents. `ok` reflects
/// whether the tool name was recognised and dispatched (not whether an
/// upstream backend the tool read from was healthy). `request_id` is the
/// raw JSON id of the request, spliced verbatim.
pub fn audit_tool_call(
  name: Name,
  tool: String,
  ok: Bool,
  request_id: String,
) -> Nil {
  let payload =
    "{\"schema\":\"dd.mcp_event.v1\",\"type\":\"mcp-tool-call\",\"source\":\"dd-gleam-mcp-server\",\"eventName\":\"tools/call\",\"severity\":\"INFO\",\"ts\":"
    <> int.to_string(now_ms())
    <> ",\"tool\":\""
    <> escape(tool)
    <> "\",\"ok\":"
    <> bool_json(ok)
    <> ",\"requestId\":"
    <> request_id
    <> "}"
  send(name, Publish(mcp_tool_events_subject, payload))
}

fn send(name: Name, message: Message) -> Nil {
  process.send(process.named_subject(name), message)
}

// ── Handler ────────────────────────────────────────────────────────────────

fn handle(state: State, msg: Message) -> actor.Next(State, Message) {
  case msg {
    Publish(subject, payload) -> {
      do_publish(state, subject, payload)
      actor.continue(state)
    }

    EmitLifecycle -> {
      do_publish(state, mcp_tool_events_subject, lifecycle_payload(state))
      actor.continue(state)
    }

    Inbound(subject, payload, headers) -> {
      case lookup_header(headers, "Source-Node") {
        Ok(node) if node == state.self_node -> actor.continue(state)
        _ -> {
          handle_control(state, subject, payload)
          actor.continue(state)
        }
      }
    }
  }
}

fn do_publish(state: State, subject: String, payload: String) -> Nil {
  let headers = [
    #(bit_array.from_string("Source-Node"), bit_array.from_string(state.self_node)),
  ]
  let _ =
    dd_nats_publish(
      state.client_pid,
      bit_array.from_string(subject),
      bit_array.from_string(payload),
      headers,
    )
  Nil
}

/// Read-only control plane. Today only `ping` is handled (a liveness echo);
/// the MCP service account is list/read only, so no command may mutate
/// cluster state.
fn handle_control(state: State, subject: String, payload: BitArray) -> Nil {
  case bit_array.to_string(payload) {
    Ok(text) ->
      case string.contains(text, "\"ping\"") {
        True -> do_publish(state, mcp_tool_events_subject, pong_payload(state))
        False -> {
          io.println(
            "dd-gleam-mcp-server nats: ignoring control message on " <> subject,
          )
          Nil
        }
      }
    Error(_) -> Nil
  }
}

fn lifecycle_payload(state: State) -> String {
  "{\"schema\":\"dd.mcp_event.v1\",\"type\":\"mcp-lifecycle\",\"source\":\"dd-gleam-mcp-server\",\"eventName\":\"started\",\"severity\":\"INFO\",\"ts\":"
  <> int.to_string(now_ms())
  <> ",\"node\":\""
  <> escape(state.self_node)
  <> "\"}"
}

fn pong_payload(state: State) -> String {
  "{\"schema\":\"dd.mcp_event.v1\",\"type\":\"mcp-control-ack\",\"source\":\"dd-gleam-mcp-server\",\"eventName\":\"pong\",\"severity\":\"INFO\",\"ts\":"
  <> int.to_string(now_ms())
  <> ",\"node\":\""
  <> escape(state.self_node)
  <> "\"}"
}

fn bool_json(value: Bool) -> String {
  case value {
    True -> "true"
    False -> "false"
  }
}

fn escape(input: String) -> String {
  input
  |> string.replace("\\", "\\\\")
  |> string.replace("\"", "\\\"")
  |> string.replace("\n", "\\n")
  |> string.replace("\r", "\\r")
  |> string.replace("\t", "\\t")
}

fn lookup_header(
  headers: List(#(String, String)),
  key: String,
) -> Result(String, Nil) {
  list.find(headers, fn(kv) { kv.0 == key })
  |> result.map(fn(kv) { kv.1 })
}

fn decode_nats_msg(raw: Dynamic) -> Message {
  // {nats_msg, Subject, Payload, Headers} — select_record passes the full
  // tuple including the tag, so 0-indexed positions are:
  //   0: nats_msg (tag)   1: Subject   2: Payload   3: Headers
  let pair = {
    use subject <- decode.field(1, decode.string)
    use payload <- decode.field(2, decode.bit_array)
    use headers <- decode.field(3, decode.list(header_decoder()))
    decode.success(#(subject, payload, headers))
  }
  case decode.run(raw, pair) {
    Ok(#(subject, payload, headers)) -> Inbound(subject, payload, headers)
    Error(e) -> {
      io.println("dd-gleam-mcp-server nats: malformed inbound: " <> string.inspect(e))
      Inbound("", <<>>, [])
    }
  }
}

fn header_decoder() -> decode.Decoder(#(String, String)) {
  use k <- decode.field(0, decode.string)
  use v <- decode.field(1, decode.string)
  decode.success(#(k, v))
}
