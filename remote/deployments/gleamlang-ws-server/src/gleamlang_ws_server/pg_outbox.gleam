//// Durable outbox tail for membership changes.
////
//// Companion to `pg_listen.gleam` — same event shape, same downstream
//// `on_event` callback, but a different transport:
////
////   - `pg_listen` decodes per-commit NOTIFYs as they arrive. Push, sub-
////     millisecond, but fire-and-forget: PG discards the NOTIFY if no
////     client happens to be LISTENing at the instant of commit.
////
////   - `pg_outbox` polls the `presence_events` table (an INSERT-only
////     monotonic sequence) for rows newer than the highest `seq` we've
////     seen so far. Survives LISTEN dropouts, pod restarts, NOTIFY
////     queue overflows. Equivalent guarantees to a WAL CDC consumer
////     using pure SQL — no replication slot, no superuser.
////
//// Both paths converge in the conversations actor, which dedupes on
//// `(conv_id, user_id, kind)` within a small window so duplicates from
//// the dual paths collapse to a single ws dispatch.
////
//// Subscription model:
////   We ref-count interest in conv and user shards exactly like
////   `pg_listen` does. The poll SQL filters by `conv_shard = ANY(...)
////   OR user_shard = ANY(...)` so each pod only pulls rows relevant to
////   its local connection set — independent of cluster size.
////
//// Wakeups:
////   - Periodic `Tick` every `tick_ms` (default 5s) — the safety net.
////   - `Wake(high_water)` — called by the `pg_listen` handler whenever
////     a NOTIFY decodes successfully. Short-circuits the poll interval
////     so a healthy LISTEN keeps the outbox lag at ~0; the periodic
////     Tick exists only to catch gaps.
////
//// Checkpoint:
////   After each successful batch we upsert the consumer's `last_seq`
////   into `presence_consumer_checkpoints` keyed by `consumer_id`
////   (defaults to `node()`). On boot the actor reads that row to know
////   where to resume from, so we don't replay history we already
////   processed.

import gleam/dict.{type Dict}
import gleam/dynamic/decode
import gleam/erlang/process.{type Subject}
import gleam/int
import gleam/io
import gleam/list
import gleam/otp/actor
import gleam/otp/supervision
import gleam/result
import gleam/string
import gleamlang_ws_server/pg_listen.{type Event as PgEvent}
import pog.{type Connection}

pub type PgOutbox =
  Subject(Message)

pub opaque type Message {
  SubscribeConv(conv_id: String)
  UnsubscribeConv(conv_id: String)
  SubscribeUser(user_id: String)
  UnsubscribeUser(user_id: String)
  /// External hint: a NOTIFY just arrived with `seq = high_water`. We
  /// know there's at least one new row to fetch; poll now rather than
  /// waiting for the next Tick.
  Wake(high_water: Int)
  Tick
  Shutdown
}

type State {
  State(
    conn: Connection,
    consumer_id: String,
    last_seq: Int,
    /// Ref-counted membership: shard -> refcount. A shard with refcount
    /// > 0 is included in the next poll's filter.
    conv_shards: Dict(Int, Int),
    user_shards: Dict(Int, Int),
    on_event: fn(PgEvent) -> Nil,
    n_shards: Int,
    tick_ms: Int,
    self: Subject(Message),
  )
}

pub fn supervised(
  conn conn: Connection,
  on_event on_event: fn(PgEvent) -> Nil,
  n_shards n_shards: Int,
  tick_ms tick_ms: Int,
) -> supervision.ChildSpecification(PgOutbox) {
  supervision.worker(fn() { start(conn, on_event, n_shards, tick_ms) })
}

pub fn start(
  conn: Connection,
  on_event: fn(PgEvent) -> Nil,
  n_shards: Int,
  tick_ms: Int,
) -> Result(actor.Started(PgOutbox), actor.StartError) {
  actor.new_with_initialiser(2000, fn(self) {
    let consumer_id = self_node_string()
    let checkpointed = read_checkpoint(conn, consumer_id)
    // First-boot behaviour: if no checkpoint exists, fast-forward to the
    // CURRENT outbox tip so we don't replay history to ws's that weren't
    // connected when those events happened. Subsequent restarts honour
    // the persisted checkpoint and DO replay anything in the gap.
    let last_seq = case checkpointed {
      0 -> {
        let tip = fetch_max_seq(conn)
        let _ = write_checkpoint(conn, consumer_id, tip)
        tip
      }
      n -> n
    }
    io.println(
      "pg_outbox: started for consumer="
      <> consumer_id
      <> " last_seq="
      <> int.to_string(last_seq),
    )
    process.send_after(self, tick_ms, Tick)
    actor.initialised(State(
      conn: conn,
      consumer_id: consumer_id,
      last_seq: last_seq,
      conv_shards: dict.new(),
      user_shards: dict.new(),
      on_event: on_event,
      n_shards: n_shards,
      tick_ms: tick_ms,
      self: self,
    ))
    |> actor.returning(self)
    |> Ok
  })
  |> actor.on_message(handle)
  |> actor.start()
}

@external(erlang, "gleamlang_ws_server_ffi", "self_node_binary")
fn self_node_string() -> String

// ── Public ref-counted shard subscription API ────────────────────────────

pub fn subscribe_conv(outbox: PgOutbox, conv_id conv_id: String) -> Nil {
  process.send(outbox, SubscribeConv(conv_id))
}

pub fn unsubscribe_conv(outbox: PgOutbox, conv_id conv_id: String) -> Nil {
  process.send(outbox, UnsubscribeConv(conv_id))
}

pub fn subscribe_user(outbox: PgOutbox, user_id user_id: String) -> Nil {
  process.send(outbox, SubscribeUser(user_id))
}

pub fn unsubscribe_user(outbox: PgOutbox, user_id user_id: String) -> Nil {
  process.send(outbox, UnsubscribeUser(user_id))
}

pub fn wake(outbox: PgOutbox, high_water high_water: Int) -> Nil {
  process.send(outbox, Wake(high_water))
}

pub fn stop(outbox: PgOutbox) -> Nil {
  process.send(outbox, Shutdown)
}

// ── Actor handler ────────────────────────────────────────────────────────

fn handle(state: State, msg: Message) -> actor.Next(State, Message) {
  case msg {
    SubscribeConv(conv_id) -> {
      let shard = pg_listen.shard_of(conv_id, state.n_shards)
      actor.continue(
        State(..state, conv_shards: bump(state.conv_shards, shard, 1)),
      )
    }
    UnsubscribeConv(conv_id) -> {
      let shard = pg_listen.shard_of(conv_id, state.n_shards)
      actor.continue(
        State(..state, conv_shards: bump(state.conv_shards, shard, -1)),
      )
    }
    SubscribeUser(user_id) -> {
      let shard = pg_listen.shard_of(user_id, state.n_shards)
      actor.continue(
        State(..state, user_shards: bump(state.user_shards, shard, 1)),
      )
    }
    UnsubscribeUser(user_id) -> {
      let shard = pg_listen.shard_of(user_id, state.n_shards)
      actor.continue(
        State(..state, user_shards: bump(state.user_shards, shard, -1)),
      )
    }

    Wake(_high_water) -> {
      // The NOTIFY arrived; the row is committed. Poll immediately.
      let new_state = poll_and_dispatch(state)
      actor.continue(new_state)
    }

    Tick -> {
      let new_state = poll_and_dispatch(state)
      process.send_after(new_state.self, new_state.tick_ms, Tick)
      actor.continue(new_state)
    }

    Shutdown -> actor.stop()
  }
}

fn bump(d: Dict(Int, Int), key: Int, delta: Int) -> Dict(Int, Int) {
  let current = dict.get(d, key) |> result.unwrap(0)
  let next = current + delta
  case next <= 0 {
    True -> dict.delete(d, key)
    False -> dict.insert(d, key, next)
  }
}

// ── Polling ──────────────────────────────────────────────────────────────

fn poll_and_dispatch(state: State) -> State {
  let conv_list = dict.keys(state.conv_shards)
  let user_list = dict.keys(state.user_shards)

  case conv_list, user_list {
    [], [] -> state
    _, _ -> {
      let rows = fetch_events(state.conn, state.last_seq, conv_list, user_list)
      case rows {
        [] -> state
        _ -> {
          // Dispatch in order; the outbox guarantees monotonic `seq`.
          list.each(rows, fn(event) { state.on_event(event) })
          let max_seq =
            list.fold(rows, state.last_seq, fn(acc, e: PgEvent) {
              case e.seq > acc {
                True -> e.seq
                False -> acc
              }
            })
          let _ = write_checkpoint(state.conn, state.consumer_id, max_seq)
          State(..state, last_seq: max_seq)
        }
      }
    }
  }
}

// ── SQL helpers ──────────────────────────────────────────────────────────

fn read_checkpoint(conn: Connection, consumer_id: String) -> Int {
  let sql =
    "select last_seq
     from presence_consumer_checkpoints
     where consumer_id = $1"
  let result =
    pog.query(sql)
    |> pog.parameter(pog.text(consumer_id))
    |> pog.returning({
      use n <- decode.field(0, decode.int)
      decode.success(n)
    })
    |> pog.execute(conn)
  case result {
    Ok(returned) ->
      case returned.rows {
        [n, ..] -> n
        [] -> 0
      }
    Error(_) -> 0
  }
}

fn write_checkpoint(
  conn: Connection,
  consumer_id: String,
  last_seq: Int,
) -> Result(Nil, String) {
  let sql =
    "insert into presence_consumer_checkpoints (consumer_id, last_seq, updated_at)
     values ($1, $2, now())
     on conflict (consumer_id) do update
       set last_seq = excluded.last_seq,
           updated_at = excluded.updated_at"
  pog.query(sql)
  |> pog.parameter(pog.text(consumer_id))
  |> pog.parameter(pog.int(last_seq))
  |> pog.execute(conn)
  |> result.map(fn(_) { Nil })
  |> result.map_error(fn(e) { string.inspect(e) })
}

fn fetch_max_seq(conn: Connection) -> Int {
  let sql = "select coalesce(max(seq), 0) from presence_events"
  let result =
    pog.query(sql)
    |> pog.returning({
      use n <- decode.field(0, decode.int)
      decode.success(n)
    })
    |> pog.execute(conn)
  case result {
    Ok(returned) ->
      case returned.rows {
        [n, ..] -> n
        [] -> 0
      }
    Error(_) -> 0
  }
}

fn fetch_events(
  conn: Connection,
  last_seq: Int,
  conv_shards: List(Int),
  user_shards: List(Int),
) -> List(PgEvent) {
  // Build a single query that filters by both axes. Empty arrays are
  // safe: `= ANY('{}')` is always false. Using ::int[] cast makes pog
  // happy without us reaching for an array-specific parameter helper.
  let sql =
    "select seq, op, conv_slug, user_slug, conv_shard, user_shard,
            soft_deleted, extract(epoch from event_at)::float8 as emitted_at
     from presence_events
     where seq > $1
       and (conv_shard = any(string_to_array($2, ',')::int[])
            or user_shard = any(string_to_array($3, ',')::int[]))
     order by seq
     limit 1000"
  let conv_csv =
    conv_shards
    |> list.map(int.to_string)
    |> string.join(",")
  let user_csv =
    user_shards
    |> list.map(int.to_string)
    |> string.join(",")
  let result =
    pog.query(sql)
    |> pog.parameter(pog.int(last_seq))
    |> pog.parameter(pog.text(conv_csv))
    |> pog.parameter(pog.text(user_csv))
    |> pog.returning(row_decoder())
    |> pog.execute(conn)
  case result {
    Ok(returned) -> returned.rows
    Error(e) -> {
      io.println("pg_outbox: fetch failed: " <> string.inspect(e))
      []
    }
  }
}

fn row_decoder() -> decode.Decoder(PgEvent) {
  use seq <- decode.field(0, decode.int)
  use op_text <- decode.field(1, decode.string)
  use conv_id <- decode.field(2, decode.string)
  use user_id <- decode.field(3, decode.string)
  use conv_shard <- decode.field(4, decode.int)
  use user_shard <- decode.field(5, decode.int)
  use soft_deleted <- decode.field(6, decode.bool)
  use emitted_at <- decode.field(7, decode.float)
  decode.success(pg_listen.Event(
    op: parse_op(op_text),
    conv_id: conv_id,
    user_id: user_id,
    soft_deleted: soft_deleted,
    conv_shard: conv_shard,
    user_shard: user_shard,
    seq: seq,
    emitted_at: emitted_at,
  ))
}

fn parse_op(s: String) -> pg_listen.Op {
  case s {
    "INSERT" -> pg_listen.OpInsert
    "UPDATE" -> pg_listen.OpUpdate
    "DELETE" -> pg_listen.OpDelete
    _ -> pg_listen.OpUpdate
  }
}
