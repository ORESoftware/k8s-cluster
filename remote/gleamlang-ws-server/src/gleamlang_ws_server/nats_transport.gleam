//// NATS transport.
////
//// Why NATS in addition to Erlang `pg`:
////   - `pg` only reaches nodes inside the BEAM cluster (epmd-discoverable
////     peers). NATS reaches every consumer of the topic on any language /
////     network — fan-in/fan-out is decoupled from BEAM clustering.
////   - NATS handles network partitions, restarts, and arbitrary
////     subscribers (Go services, web gateways, log shippers) without us
////     having to add them to the cluster.
////   - It also acts as a redundant transport: when BEAM dist or the
////     LISTEN/NOTIFY path fails, broadcasts still get through via NATS.
////
//// Subject layout:
////   presence.broadcast.<conv_id>         — broadcasts to a conv
////   presence.member_change.<conv_id>     — membership changes (optional;
////                                          currently we use PG NOTIFY)
////
//// Source-node dedup:
////   Every PUB carries a `Source-Node` header set to the BEAM node atom.
////   On receive, we drop messages whose Source-Node equals our own node,
////   so the round-trip through NATS doesn't re-deliver locally — local
////   delivery already happened via the ETS registry inside fanout.
////
//// Reconnect:
////   The underlying `dd_nats` Erlang gen_server reconnects with jittered
////   backoff (1s → 30s). In-flight subscriptions are replayed on
////   reconnect, so callers don't need to re-subscribe.

import gleam/bit_array
import gleam/dynamic.{type Dynamic}
import gleam/dynamic/decode
import gleam/erlang/atom
import gleam/erlang/process.{type Pid, type Subject}
import gleam/io
import gleam/list
import gleam/option.{type Option, Some}
import gleam/otp/actor
import gleam/otp/supervision
import gleam/result
import gleam/string
import gleamlang_ws_server/fanout.{type Fanout}
import gleamlang_ws_server/groups.{
  type ConnGroup, type ConnMsg, ByConv, Outbound,
}
import gleamlang_ws_server/registry.{type Registry}

pub type Nats =
  Subject(Message)

pub opaque type Message {
  Publish(subject: String, payload: BitArray, headers: List(#(String, String)))
  Subscribe(subject: String)
  Raw(subject: String, payload: BitArray, headers: List(#(String, String)))
  Shutdown
}

/// A decoded inbound NATS message with source-node already dropped on
/// match. `subject` is the actual subject the message was published to
/// (after any wildcard expansion).
pub type Inbound {
  Inbound(
    subject: String,
    payload: BitArray,
    headers: List(#(String, String)),
    source_node: Option(String),
  )
}

type State {
  State(
    client_pid: Pid,
    on_msg: fn(Inbound) -> Nil,
    self_node: String,
  )
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

@external(erlang, "gleamlang_ws_server_ffi", "self_node_binary")
fn self_node_binary() -> String

// ── Public API ───────────────────────────────────────────────────────────

pub fn supervised(
  url url: String,
  on_msg on_msg: fn(Inbound) -> Nil,
) -> supervision.ChildSpecification(Nats) {
  supervision.worker(fn() { start(url, on_msg) })
}

pub fn start(
  url: String,
  on_msg: fn(Inbound) -> Nil,
) -> Result(actor.Started(Nats), actor.StartError) {
  actor.new_with_initialiser(2000, fn(self) {
    let self_pid = process.self()
    case dd_nats_start_link(url, self_pid) {
      Error(e) -> Error("dd_nats: " <> string.inspect(e))
      Ok(client) -> {
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
          on_msg: on_msg,
          self_node: self_node_binary(),
        ))
        |> actor.selecting(selector)
        |> actor.returning(self)
        |> Ok
      }
    }
  })
  |> actor.on_message(handle)
  |> actor.start()
}

pub fn publish(
  nats: Nats,
  subject subject: String,
  payload payload: BitArray,
  headers headers: List(#(String, String)),
) -> Nil {
  process.send(nats, Publish(subject, payload, headers))
}

pub fn subscribe(nats: Nats, subject subject: String) -> Nil {
  process.send(nats, Subscribe(subject))
}

pub fn stop(nats: Nats) -> Nil {
  process.send(nats, Shutdown)
}

/// Build the NATS subject for a conv broadcast. Stable so external
/// publishers can target a conv directly without going through the HTTP
/// API.
pub fn conv_subject(conv_id: String) -> String {
  "presence.broadcast.conv." <> conv_id
}

/// Extract the `<conv_id>` suffix from a `presence.broadcast.conv.<id>`
/// subject. Returns Error if the subject doesn't match.
pub fn conv_id_from_subject(subject: String) -> Result(String, Nil) {
  let prefix = "presence.broadcast.conv."
  case string.starts_with(subject, prefix) {
    True -> Ok(string.drop_start(subject, string.length(prefix)))
    False -> Error(Nil)
  }
}

/// Helper to filter out same-cluster source nodes. When `source_node` is
/// in the current BEAM cluster's `erlang:nodes()`, the pg-mesh path has
/// already delivered locally — we drop the NATS copy to avoid double-
/// fire.
@external(erlang, "erlang", "nodes")
fn erlang_nodes() -> List(atom.Atom)

pub fn source_node_is_cluster_peer(source_node: Option(String)) -> Bool {
  case source_node {
    Some(node_bin) -> {
      // `atom.get` returns `Ok(atom)` only when the atom already exists.
      // If we've never heard of this node, it can't be a cluster peer.
      case atom.get(node_bin) {
        Ok(node_atom) -> list.contains(erlang_nodes(), node_atom)
        Error(_) -> False
      }
    }
    _ -> False
  }
}

/// Generic dispatch for an inbound `presence.broadcast.conv.<id>` packet:
/// drop if it came from a cluster peer (pg-mesh already delivered it),
/// otherwise dispatch as `Outbound` to `ByConv(conv_id)` via the local
/// registry.
///
/// The payload is treated as an opaque UTF-8 string (the websocket
/// frame). This is the right shape for `Outbound` messages — other
/// `ConnMsg` variants are server-internal and intentionally NOT bridged
/// over NATS.
pub fn dispatch_inbound_default(
  inbound: Inbound,
  reg: Registry(ConnMsg, ConnGroup),
  _fan: Fanout,
) -> Nil {
  case source_node_is_cluster_peer(inbound.source_node) {
    True -> Nil
    False -> {
      case conv_id_from_subject(inbound.subject) {
        Error(_) -> Nil
        Ok(conv_id) ->
          case bit_array.to_string(inbound.payload) {
            Error(_) -> Nil
            Ok(payload_str) -> {
              registry.dispatch_group(reg, ByConv(conv_id), fn(subj) {
                process.send(subj, Outbound(payload_str))
                Nil
              })
              Nil
            }
          }
      }
    }
  }
}

// ── Handler ──────────────────────────────────────────────────────────────

fn handle(state: State, msg: Message) -> actor.Next(State, Message) {
  case msg {
    Publish(subject, payload, headers) -> {
      let headers_with_source = [
        #("Source-Node", state.self_node),
        ..headers
      ]
      let subj_bin = bit_array.from_string(subject)
      let hdr_bins =
        list.map(headers_with_source, fn(kv) {
          #(bit_array.from_string(kv.0), bit_array.from_string(kv.1))
        })
      let _ = dd_nats_publish(state.client_pid, subj_bin, payload, hdr_bins)
      actor.continue(state)
    }

    Subscribe(subject) -> {
      let _ =
        dd_nats_subscribe(state.client_pid, bit_array.from_string(subject))
      actor.continue(state)
    }

    Raw(subject, payload, headers) -> {
      let source_node = lookup_header(headers, "Source-Node")
      case source_node {
        Some(node) if node == state.self_node -> {
          // Self-originated; skip — local delivery already happened.
          actor.continue(state)
        }
        _ -> {
          state.on_msg(Inbound(
            subject: subject,
            payload: payload,
            headers: headers,
            source_node: source_node,
          ))
          actor.continue(state)
        }
      }
    }

    Shutdown -> actor.stop()
  }
}

fn lookup_header(
  headers: List(#(String, String)),
  key: String,
) -> Option(String) {
  list.find(headers, fn(kv) { kv.0 == key })
  |> result.map(fn(kv) { kv.1 })
  |> option.from_result
}

fn decode_nats_msg(raw: Dynamic) -> Message {
  // {nats_msg, Subject, Payload, Headers} — 4-tuple. select_record passes
  // the full tuple including the tag, so 0-indexed positions are:
  //   0: nats_msg (tag)   1: Subject   2: Payload   3: Headers
  let pair = {
    use subject <- decode.field(1, decode.string)
    use payload <- decode.field(2, decode.bit_array)
    use headers <- decode.field(3, decode.list(header_decoder()))
    decode.success(#(subject, payload, headers))
  }
  case decode.run(raw, pair) {
    Ok(#(subject, payload, headers)) -> Raw(subject, payload, headers)
    Error(e) -> {
      io.println("nats: malformed inbound: " <> string.inspect(e))
      Raw("", <<>>, [])
    }
  }
}

fn header_decoder() -> decode.Decoder(#(String, String)) {
  use k <- decode.field(0, decode.string)
  use v <- decode.field(1, decode.string)
  decode.success(#(k, v))
}
