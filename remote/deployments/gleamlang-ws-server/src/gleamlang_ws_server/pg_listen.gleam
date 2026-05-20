//// Sharded PG LISTEN/NOTIFY.
////
//// Per-pod actor that manages a dedicated `pgo_notifications` connection
//// (which `pgo` keeps alive via its own internal backoff/reconnect loop).
////
//// Subscriptions are ref-counted per shard so we can incrementally
//// LISTEN / UNLISTEN as the pod's conversation set grows and shrinks.
////
//// Public API:
////   start/3, supervised/3 — boot the listener; pass in the pgo
////     `pool_config` map and a forwarder `Subject(Event)` that the
////     module emits decoded events to (typically the conversations
////     actor).
////   subscribe(self, conv_id)   — bump the shard's ref count by 1.
////   unsubscribe(self, conv_id) — decrement; UNLISTEN when count = 0.
////
//// pgo_notifications delivers `{notification, ServerPid, Ref, Channel,
//// Payload}` as a raw Erlang message. We pick those up via
//// `process.select_record` and emit a typed `Event` to the forwarder.

import gleam/dict.{type Dict}
import gleam/dynamic.{type Dynamic}
import gleam/dynamic/decode
import gleam/erlang/atom.{type Atom}
import gleam/erlang/process.{type Pid, type Subject}
import gleam/int
import gleam/io
import gleam/json
import gleam/option.{type Option, Some}
import gleam/otp/actor
import gleam/otp/supervision
import gleam/result
import gleam/string

pub type Op {
  OpInsert
  OpUpdate
  OpDelete
}

pub type Event {
  Event(
    op: Op,
    conv_id: String,
    user_id: String,
    soft_deleted: Bool,
    conv_shard: Int,
    user_shard: Int,
    seq: Int,
    emitted_at: Float,
  )
}

/// Which sharding axis a subscription is on. Both axes share the same
/// numeric shard space; they differ only in the channel name prefix
/// (`presence_change_conv_<n>` vs `presence_change_user_<n>`).
pub type Axis {
  ConvAxis
  UserAxis
}

pub type PgListen =
  Subject(Message)

pub opaque type Message {
  Subscribe(axis: Axis, id: String)
  Unsubscribe(axis: Axis, id: String)
  Raw(channel: String, payload: String)
  Shutdown
}

type State {
  State(
    server: Pid,
    /// (axis, shard) -> { ref count, opaque pgo listen reference }
    shards: Dict(#(Axis, Int), ShardState),
    on_event: fn(Event) -> Nil,
    n_shards: Int,
  )
}

type ShardState {
  ShardState(refcount: Int, listen_ref: Dynamic)
}

// ── pgo_notifications FFI ────────────────────────────────────────────────

@external(erlang, "pgo_notifications", "start_link")
fn pgo_notifications_start_link(config: PgoConfig) -> Result(Pid, Dynamic)

@external(erlang, "pgo_notifications", "listen")
fn pgo_notifications_listen(
  server: Pid,
  channel: String,
) -> Result(Dynamic, Dynamic)

@external(erlang, "pgo_notifications", "unlisten")
fn pgo_notifications_unlisten(server: Pid, ref: Dynamic) -> Atom

/// `pgo` config map: `#{host => "...", port => 5432, user => "...",
/// password => "...", database => "..."}`. Opaque to Gleam.
pub type PgoConfig

@external(erlang, "gleamlang_ws_server_ffi", "pgo_config")
pub fn pgo_config(
  host: String,
  port: Int,
  user: String,
  password: String,
  database: String,
) -> PgoConfig

@external(erlang, "gleamlang_ws_server_ffi", "pgo_config_from_url")
pub fn pgo_config_from_url(url: String) -> Result(PgoConfig, String)

// ── Public API ───────────────────────────────────────────────────────────

pub fn supervised(
  config config: PgoConfig,
  on_event on_event: fn(Event) -> Nil,
  n_shards n_shards: Int,
) -> supervision.ChildSpecification(PgListen) {
  supervision.worker(fn() { start(config, on_event, n_shards) })
}

pub fn start(
  config: PgoConfig,
  on_event: fn(Event) -> Nil,
  n_shards: Int,
) -> Result(actor.Started(PgListen), actor.StartError) {
  actor.new_with_initialiser(2000, fn(self) {
    case pgo_notifications_start_link(config) {
      Error(e) -> Error(string.inspect(e))
      Ok(server) -> {
        // Selector: handle our own typed messages + the raw 5-tuple
        // {notification, ServerPid, Ref, Channel, Payload} that
        // pgo_notifications emits.
        let selector =
          process.new_selector()
          |> process.select(self)
          |> process.select_record(
            tag: atom.create("notification"),
            fields: 4,
            mapping: decode_notification,
          )
        actor.initialised(State(
          server: server,
          shards: dict.new(),
          on_event: on_event,
          n_shards: n_shards,
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

/// Subscribe to the conv-axis shard for `conv_id`. Idempotent: ref-
/// counted, so calling subscribe(N) then unsubscribe(N-1) leaves us
/// LISTENing.
pub fn subscribe_conv(listen: PgListen, conv_id conv_id: String) -> Nil {
  process.send(listen, Subscribe(ConvAxis, conv_id))
}

pub fn unsubscribe_conv(listen: PgListen, conv_id conv_id: String) -> Nil {
  process.send(listen, Unsubscribe(ConvAxis, conv_id))
}

/// Subscribe to the user-axis shard for `user_id`. Used so a pod with a
/// user-scope ws gets notified about that user being added to any new
/// conv, regardless of whether the pod also listens on the conv's shard.
pub fn subscribe_user(listen: PgListen, user_id user_id: String) -> Nil {
  process.send(listen, Subscribe(UserAxis, user_id))
}

pub fn unsubscribe_user(listen: PgListen, user_id user_id: String) -> Nil {
  process.send(listen, Unsubscribe(UserAxis, user_id))
}

pub fn stop(listen: PgListen) -> Nil {
  process.send(listen, Shutdown)
}

/// Compute the shard a conv belongs to, locally, mirroring the SQL
/// hash. Lets the conversations actor (and tests) reason about which
/// channel a given conv will be NOTIFY'd on.
pub fn shard_of(conv_id: String, n_shards: Int) -> Int {
  // Mirrors the SQL trigger `notify_presence_member_change()` in
  // schema.sql: take the first 4 hex digits of the canonical UUID form
  // (after stripping hyphens), interpret as an unsigned 16-bit integer,
  // then modulo n_shards. Non-UUID conv_ids fall back to `phash2` purely
  // so the listener doesn't crash on demo data — the SQL side also
  // bails on such inputs, so the two paths simply never converge for
  // non-UUID conv_ids and that's fine. If the algorithm ever drifts,
  // cross-check with `select presence_shard_of('<uuid>')`.
  shard_of_ffi(conv_id, n_shards)
}

@external(erlang, "gleamlang_ws_server_ffi", "shard_of")
fn shard_of_ffi(conv_id: String, n_shards: Int) -> Int

// ── Actor handler ────────────────────────────────────────────────────────

fn handle(state: State, msg: Message) -> actor.Next(State, Message) {
  case msg {
    Subscribe(axis, id) -> {
      let key = #(axis, shard_of(id, state.n_shards))
      let new_state = case dict.get(state.shards, key) {
        Ok(existing) ->
          State(
            ..state,
            shards: dict.insert(
              state.shards,
              key,
              ShardState(..existing, refcount: existing.refcount + 1),
            ),
          )
        Error(_) -> {
          let channel = channel_name(key)
          case pgo_notifications_listen(state.server, channel) {
            Ok(ref) -> {
              io.println("pg_listen: LISTEN " <> channel)
              State(
                ..state,
                shards: dict.insert(
                  state.shards,
                  key,
                  ShardState(refcount: 1, listen_ref: ref),
                ),
              )
            }
            Error(e) -> {
              io.println(
                "pg_listen: LISTEN failed for "
                <> channel
                <> ": "
                <> string.inspect(e),
              )
              state
            }
          }
        }
      }
      actor.continue(new_state)
    }

    Unsubscribe(axis, id) -> {
      let key = #(axis, shard_of(id, state.n_shards))
      let new_state = case dict.get(state.shards, key) {
        Error(_) -> state
        Ok(existing) -> {
          let next_count = existing.refcount - 1
          case next_count <= 0 {
            False ->
              State(
                ..state,
                shards: dict.insert(
                  state.shards,
                  key,
                  ShardState(..existing, refcount: next_count),
                ),
              )
            True -> {
              let _ =
                pgo_notifications_unlisten(state.server, existing.listen_ref)
              io.println("pg_listen: UNLISTEN " <> channel_name(key))
              State(..state, shards: dict.delete(state.shards, key))
            }
          }
        }
      }
      actor.continue(new_state)
    }

    Raw(_channel, payload) -> {
      case decode_event(payload) {
        Ok(event) -> state.on_event(event)
        Error(reason) ->
          io.println("pg_listen: malformed payload: " <> reason)
      }
      actor.continue(state)
    }

    Shutdown -> actor.stop()
  }
}

fn channel_name(key: #(Axis, Int)) -> String {
  let #(axis, shard) = key
  let prefix = case axis {
    ConvAxis -> "presence_change_conv_"
    UserAxis -> "presence_change_user_"
  }
  prefix <> int.to_string(shard)
}

// ── Dynamic / JSON decoding ──────────────────────────────────────────────

fn decode_notification(raw: Dynamic) -> Message {
  // pgo_notifications sends `{notification, ServerPid, Ref, Channel,
  // Payload}` — a 5-tuple. `select_record` passes the full tuple to us
  // (including the tag), so the 0-indexed positions are:
  //   0: notification (tag)    1: ServerPid    2: Ref
  //   3: Channel               4: Payload
  let pair_decoder = {
    use channel <- decode.field(3, decode.string)
    use payload <- decode.field(4, decode.string)
    decode.success(#(channel, payload))
  }
  case decode.run(raw, pair_decoder) {
    Ok(#(channel, payload)) -> Raw(channel, payload)
    Error(_) -> Raw("?", "")
  }
}

fn decode_event(json_str: String) -> Result(Event, String) {
  let decoder = {
    use op <- decode.field("op", decode.string)
    use conv_id <- decode.field("conv_id", decode.string)
    use user_id <- decode.field("user_id", decode.string)
    use soft_deleted <- decode.field("soft_deleted", decode.bool)
    use conv_shard <- decode.field("conv_shard", decode.int)
    use user_shard <- decode.field("user_shard", decode.int)
    use seq <- decode.field("seq", decode.int)
    use emitted_at <- decode.field("emitted_at", decode.float)
    decode.success(Event(
      op: parse_op(op),
      conv_id: conv_id,
      user_id: user_id,
      soft_deleted: soft_deleted,
      conv_shard: conv_shard,
      user_shard: user_shard,
      seq: seq,
      emitted_at: emitted_at,
    ))
  }
  json.parse(from: json_str, using: decoder)
  |> result.map_error(fn(e) { "json: " <> string.inspect(e) })
}

fn parse_op(op: String) -> Op {
  case op {
    "INSERT" -> OpInsert
    "UPDATE" -> OpUpdate
    "DELETE" -> OpDelete
    _ -> OpUpdate
  }
}

/// Map an event into the conversations actor's `PeerEcho` semantics. An
/// INSERT or UPDATE with `soft_deleted = false` is a join; an UPDATE with
/// `soft_deleted = true` is a leave; a DELETE is also a leave.
pub type SemanticKind {
  KindAdded
  KindRemoved
}

pub fn semantic_kind(event: Event) -> Option(SemanticKind) {
  case event.op, event.soft_deleted {
    OpInsert, False -> Some(KindAdded)
    OpInsert, True -> Some(KindRemoved)
    OpUpdate, False -> Some(KindAdded)
    OpUpdate, True -> Some(KindRemoved)
    OpDelete, _ -> Some(KindRemoved)
  }
}
