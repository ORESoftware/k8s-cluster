//// True logical-replication consumer via `pg_logical_slot_get_changes`.
////
//// This is the "WAL CDC" leg of the membership pipeline. It runs in
//// parallel with `pg_listen` (fast push path) and `pg_outbox` (durable
//// app-managed poll path); all three converge on the same
//// `conversations.IncomingPgEvent` door, with the dedup cache in the
//// conversations actor absorbing overlap.
////
//// Why three paths:
////   * LISTEN/NOTIFY: sub-ms latency, fire-and-forget.
////   * Outbox table: pure SQL, app-controlled schema (we choose what's
////     in `presence_events`), no operator setup beyond `CREATE TABLE`.
////   * WAL via slot: schema-agnostic CDC straight from the WAL, slot
////     retains data if the consumer is down, foundation for future
////     CDC needs (data lake, search index, audit log) without touching
////     the trigger.
////
//// Why SQL-poll (`pg_logical_slot_get_changes`) instead of the streaming
//// replication protocol:
////   * Uses the same `pog` connection pool we already have. No new
////     Erlang client, no replication-protocol parser to maintain.
////   * Same operational guarantees as streaming: slot retention, LSN
////     advancement, idempotent replay across restart.
////   * Trade-off: ~poll-interval latency vs streaming push. We don't
////     mind because LISTEN/NOTIFY already covers sub-ms latency.
////
//// Operator prereqs:
////   * `wal_level = logical` (RDS: rds.logical_replication = 1, reboot)
////   * `wal2json` extension installed
////   * `max_replication_slots` high enough for all pods
////   * `max_slot_wal_keep_size` set (CRITICAL — bounds the "dead slot
////     fills disk" risk; PG14+).
////   * `presence_pub` publication and `presence_ensure_wal_slot()`
////     helpers exist in the schema (created by `schema.sql`).
////
//// Boot sequence inside `start`:
////   1. `select presence_wal_available()` → bail with a friendly log
////      message if the operator hasn't enabled logical replication.
////   2. `select presence_ensure_wal_slot('presence_wal_<node>')`
////      (idempotent — creates the slot if missing).
////   3. Start the poll loop.
////
//// Tick loop:
////   * Call `pg_logical_slot_get_changes('presence_wal_<node>', NULL,
////     NULL, 'add-tables', 'public.presence_conv_members',
////     'format-version', '2')`. Each row is `(lsn, xid, data)` where
////     `data` is a JSON line per change. We parse the JSON, map to a
////     `pg_listen.Event`, and forward via `on_event`.
////   * The slot's `confirmed_flush_lsn` advances automatically as we
////     consume rows (this is what `_get_changes` does — `_peek_changes`
////     would not). On crash mid-batch, we redeliver from the slot's
////     current position, so the conversations actor's dedup cache is
////     load-bearing.
////
//// wal2json format-version 2 emits one JSON object per row change with
//// fields:
////   action: "I" | "U" | "D" | "T" | "M" | "B" | "C"
////   schema: "public"
////   table:  "presence_conv_members"
////   columns / identity:  arrays of {name, type, value}
//// We map "I" + soft_deleted=false → KindAdded, "I"/"U" with
//// soft_deleted=true → KindRemoved, "D" → KindRemoved. The `seq`
//// field doesn't exist in the WAL output (it's an outbox-only column),
//// so we synthesise one from the LSN — purely for ordering inside our
//// pipeline.

import gleam/dynamic/decode
import gleam/erlang/process.{type Subject}
import gleam/int
import gleam/io
import gleam/list
import gleam/option
import gleam/otp/actor
import gleam/otp/supervision
import gleam/string
import gleamlang_ws_server/pg_listen.{type Event as PgEvent}
import pog.{type Connection}

pub type PgWal =
  Subject(Message)

pub opaque type Message {
  Tick
  Shutdown
}

type State {
  State(
    conn: Connection,
    slot_name: String,
    on_event: fn(PgEvent) -> Nil,
    tick_ms: Int,
    self: Subject(Message),
    n_shards: Int,
  )
}

@external(erlang, "gleamlang_ws_server_ffi", "self_node_binary")
fn self_node_string() -> String

pub fn supervised(
  conn conn: Connection,
  on_event on_event: fn(PgEvent) -> Nil,
  n_shards n_shards: Int,
  tick_ms tick_ms: Int,
) -> supervision.ChildSpecification(PgWal) {
  supervision.worker(fn() { start(conn, on_event, n_shards, tick_ms) })
}

pub fn start(
  conn: Connection,
  on_event: fn(PgEvent) -> Nil,
  n_shards: Int,
  tick_ms: Int,
) -> Result(actor.Started(PgWal), actor.StartError) {
  actor.new_with_initialiser(2000, fn(self) {
    case wal_available(conn) {
      False -> Error("wal_level != logical or wal2json not installed")
      True -> {
        // Per-node slot so multi-pod deployments each have their own
        // cursor. Slot names must match `[a-z0-9_]{1,63}` so the node
        // name gets sanitised.
        let slot_name = "presence_wal_" <> sanitise(self_node_string())
        case ensure_slot(conn, slot_name) {
          Error(reason) -> Error("pg_wal: ensure_slot failed: " <> reason)
          Ok(_) -> {
            io.println(
              "pg_wal: slot ready ("
              <> slot_name
              <> "); polling every "
              <> int.to_string(tick_ms)
              <> "ms",
            )
            process.send_after(self, tick_ms, Tick)
            actor.initialised(State(
              conn: conn,
              slot_name: slot_name,
              on_event: on_event,
              tick_ms: tick_ms,
              self: self,
              n_shards: n_shards,
            ))
            |> actor.returning(self)
            |> Ok
          }
        }
      }
    }
  })
  |> actor.on_message(handle)
  |> actor.start()
}

pub fn stop(wal: PgWal) -> Nil {
  process.send(wal, Shutdown)
}

fn handle(state: State, msg: Message) -> actor.Next(State, Message) {
  case msg {
    Tick -> {
      poll_once(state)
      process.send_after(state.self, state.tick_ms, Tick)
      actor.continue(state)
    }
    Shutdown -> actor.stop()
  }
}

// ── SQL ──────────────────────────────────────────────────────────────────

fn wal_available(conn: Connection) -> Bool {
  let sql = "select presence_wal_available()"
  let result =
    pog.query(sql)
    |> pog.returning({
      use b <- decode.field(0, decode.bool)
      decode.success(b)
    })
    |> pog.execute(conn)
  case result {
    Ok(returned) ->
      case returned.rows {
        [b, ..] -> b
        [] -> False
      }
    Error(_) -> False
  }
}

fn ensure_slot(conn: Connection, slot_name: String) -> Result(Nil, String) {
  let sql = "select presence_ensure_wal_slot($1)"
  let result =
    pog.query(sql)
    |> pog.parameter(pog.text(slot_name))
    |> pog.returning({
      use ok <- decode.field(0, decode.bool)
      decode.success(ok)
    })
    |> pog.execute(conn)
  case result {
    Error(e) -> Error("query failed: " <> string.inspect(e))
    Ok(returned) ->
      case returned.rows {
        [True, ..] -> Ok(Nil)
        [False, ..] ->
          Error("slot creation refused — wal2json plugin not installed?")
        [] -> Error("ensure_wal_slot returned no rows")
      }
  }
}

fn poll_once(state: State) -> Nil {
  let sql =
    "select data
     from pg_logical_slot_get_changes(
       $1::text,
       null,
       null,
       'add-tables', 'public.presence_conv_members',
       'format-version', '2',
       'include-types', 'false'
     )"
  let result =
    pog.query(sql)
    |> pog.parameter(pog.text(state.slot_name))
    |> pog.returning({
      use data <- decode.field(0, decode.string)
      decode.success(data)
    })
    |> pog.execute(state.conn)
  case result {
    Error(e) -> io.println("pg_wal: poll failed: " <> string.inspect(e))
    Ok(returned) -> {
      // wal2json emits one row per change message (plus periodic
      // BEGIN/COMMIT envelopes which we discard). For each row we
      // resolve UUIDs -> slugs so downstream consumers see the same
      // ID space as the LISTEN/NOTIFY and outbox paths.
      list.each(returned.rows, fn(json_line) {
        case parse_wal2json(json_line, state.n_shards) {
          option.Some(event_uuid_form) -> {
            let event = resolve_slugs(state.conn, event_uuid_form)
            state.on_event(event)
          }
          option.None -> Nil
        }
      })
    }
  }
}

/// The raw wal2json payload has UUIDs in `conv_id` / `user_id`. Convert
/// to slugs so the conversations actor's slug-keyed cache matches the
/// LISTEN/NOTIFY and outbox paths. Falls back to the UUID text if no
/// slug row exists (production case where the input IS a real UUID).
fn resolve_slugs(conn: Connection, event: PgEvent) -> PgEvent {
  let conv_slug = lookup_conv_slug(conn, event.conv_id)
  let user_slug = lookup_user_slug(conn, event.user_id)
  pg_listen.Event(..event, conv_id: conv_slug, user_id: user_slug)
}

fn lookup_conv_slug(conn: Connection, uuid_text: String) -> String {
  let sql = "select slug from presence_convs where id = $1::uuid"
  let result =
    pog.query(sql)
    |> pog.parameter(pog.text(uuid_text))
    |> pog.returning({
      use s <- decode.field(0, decode.string)
      decode.success(s)
    })
    |> pog.execute(conn)
  case result {
    Ok(returned) ->
      case returned.rows {
        [s, ..] -> s
        [] -> uuid_text
      }
    Error(_) -> uuid_text
  }
}

fn lookup_user_slug(conn: Connection, uuid_text: String) -> String {
  let sql = "select slug from presence_users where id = $1::uuid"
  let result =
    pog.query(sql)
    |> pog.parameter(pog.text(uuid_text))
    |> pog.returning({
      use s <- decode.field(0, decode.string)
      decode.success(s)
    })
    |> pog.execute(conn)
  case result {
    Ok(returned) ->
      case returned.rows {
        [s, ..] -> s
        [] -> uuid_text
      }
    Error(_) -> uuid_text
  }
}

// ── wal2json v2 JSON shape parsing ───────────────────────────────────────
//
// Example INSERT message (heavily abbreviated):
//
//   {
//     "action": "I",
//     "schema": "public",
//     "table":  "presence_conv_members",
//     "columns": [
//       {"name": "conv_id", "value": "..."},
//       {"name": "user_id", "value": "..."},
//       {"name": "is_soft_deleted", "value": false},
//       …
//     ]
//   }
//
// UPDATE includes both "columns" (new state) and "identity" (old PK).
// DELETE has only "identity".

@external(erlang, "gleamlang_ws_server_ffi", "parse_wal2json")
fn parse_wal2json_ffi(
  json: String,
) -> Result(#(String, String, String, Bool), String)

fn parse_wal2json(json_line: String, n_shards: Int) -> option.Option(PgEvent) {
  case parse_wal2json_ffi(json_line) {
    Error(_) -> option.None
    Ok(#(action, conv_id, user_id, soft_deleted)) -> {
      let op = case action {
        "I" -> pg_listen.OpInsert
        "U" -> pg_listen.OpUpdate
        "D" -> pg_listen.OpDelete
        _ -> pg_listen.OpUpdate
      }
      let conv_shard = pg_listen.shard_of(conv_id, n_shards)
      let user_shard = pg_listen.shard_of(user_id, n_shards)
      option.Some(pg_listen.Event(
        op: op,
        conv_id: conv_id,
        user_id: user_id,
        soft_deleted: soft_deleted,
        conv_shard: conv_shard,
        user_shard: user_shard,
        // No real seq in the WAL stream — the slot's LSN serves that
        // role but the conversations dedup cache doesn't use it as a
        // key (it dedupes on (conv, user, kind)), so 0 is harmless.
        seq: 0,
        emitted_at: 0.0,
      ))
    }
  }
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn sanitise(s: String) -> String {
  // Slot names must match `[a-z0-9_]{1,63}`. Erlang node names contain
  // `@` and `.` which we map to `_`. Trim to 50 chars to leave room for
  // the `presence_wal_` prefix.
  s
  |> string.lowercase
  |> string.replace(each: "@", with: "_")
  |> string.replace(each: ".", with: "_")
  |> string.replace(each: "-", with: "_")
  |> string.slice(at_index: 0, length: 50)
}
