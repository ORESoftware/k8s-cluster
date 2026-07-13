//// Postgres-backed durable membership.
////
//// This module is the **source of truth** for "is user X a member of conv
//// Y?". The in-memory cache in `conversations.gleam` is a lazy
//// materialisation of the subset of these rows that have at least one
//// live connection on this node.
////
//// Tables (defined in `remote/libs/pg-defs/schema/schema.sql`, regenerated
//// adapter at `dd_pg_defs.gleam`):
////
////   * `presence_convs`         — id, slug, status, …
////   * `presence_conv_members`  — conv_id, user_id, role, status, …
////
//// The store supports two modes:
////
////   - `Configured(connection)` — backed by a `pog` connection pool.
////   - `InMemory(cache)`        — for local dev / unit tests / cluster
////                                bring-up before a database is wired in.
////                                Behaviour-compatible with the real one.
////
//// `InMemory` is decided at boot time by inspecting the `PG_DATABASE_URL`
//// env var. If absent, we run the demo with an in-process map. If present,
//// pog opens the pool.

import gleam/dict.{type Dict}
import gleam/dynamic/decode
import gleam/erlang/process.{type Subject}
import gleam/option.{type Option}
import gleam/otp/actor
import gleam/otp/supervision
import gleam/result
import gleam/set.{type Set}
import gleam/string
import gleamlang_presence_server/groups.{type ConvId, type UserId}
import pog.{type Connection}

pub type Mode {
  Configured(connection: Connection)
  InMemory(actor: Subject(MemMsg))
}

pub type Store {
  Store(mode: Mode)
}

// ── In-memory fallback actor ─────────────────────────────────────────────

pub opaque type MemMsg {
  MemAdd(conv: ConvId, user: UserId, reply: Subject(Result(Nil, String)))
  MemRemove(conv: ConvId, user: UserId, reply: Subject(Result(Nil, String)))
  MemMembers(conv: ConvId, reply: Subject(List(UserId)))
  MemConvsOf(user: UserId, reply: Subject(List(ConvId)))
}

type MemState {
  MemState(
    conv_members: Dict(ConvId, Set(UserId)),
    user_convs: Dict(UserId, Set(ConvId)),
  )
}

fn start_mem() -> Result(actor.Started(Subject(MemMsg)), actor.StartError) {
  actor.new(MemState(conv_members: dict.new(), user_convs: dict.new()))
  |> actor.on_message(handle_mem)
  |> actor.start()
}

/// Start an in-memory store directly, linked to the caller. Useful when no
/// `PG_DATABASE_URL` is configured — the caller wants a synchronous
/// "ready" `Store` value without building a supervisor tree first.
pub fn start_inmemory() -> Result(Store, actor.StartError) {
  use started <- result.try(start_mem())
  Ok(Store(mode: InMemory(actor: started.data)))
}

/// Start a Postgres-backed store directly, linked to the caller. The
/// caller passes a pog config URL; we open the pool and return a `Store`.
pub fn start_postgres(url: String) -> Result(Store, actor.StartError) {
  let config_result =
    pog.url_config(process.new_name(prefix: "presence_pog"), url)
  case config_result {
    Ok(cfg) -> {
      use started <- result.try(pog.start(cfg))
      Ok(Store(mode: Configured(connection: started.data)))
    }
    Error(_) -> Error(actor.InitFailed("invalid PG_DATABASE_URL"))
  }
}

fn handle_mem(
  state: MemState,
  message: MemMsg,
) -> actor.Next(MemState, MemMsg) {
  case message {
    MemAdd(conv, user, reply) -> {
      let convs =
        dict.get(state.conv_members, conv)
        |> result.unwrap(set.new())
        |> set.insert(user)
      let user_convs =
        dict.get(state.user_convs, user)
        |> result.unwrap(set.new())
        |> set.insert(conv)
      process.send(reply, Ok(Nil))
      actor.continue(MemState(
        conv_members: dict.insert(state.conv_members, conv, convs),
        user_convs: dict.insert(state.user_convs, user, user_convs),
      ))
    }
    MemRemove(conv, user, reply) -> {
      let conv_set =
        dict.get(state.conv_members, conv)
        |> result.unwrap(set.new())
        |> set.delete(user)
      let user_set =
        dict.get(state.user_convs, user)
        |> result.unwrap(set.new())
        |> set.delete(conv)
      let conv_members = case set.is_empty(conv_set) {
        True -> dict.delete(state.conv_members, conv)
        False -> dict.insert(state.conv_members, conv, conv_set)
      }
      let user_convs = case set.is_empty(user_set) {
        True -> dict.delete(state.user_convs, user)
        False -> dict.insert(state.user_convs, user, user_set)
      }
      process.send(reply, Ok(Nil))
      actor.continue(MemState(
        conv_members: conv_members,
        user_convs: user_convs,
      ))
    }
    MemMembers(conv, reply) -> {
      let users =
        dict.get(state.conv_members, conv)
        |> result.unwrap(set.new())
        |> set.to_list
      process.send(reply, users)
      actor.continue(state)
    }
    MemConvsOf(user, reply) -> {
      let convs =
        dict.get(state.user_convs, user)
        |> result.unwrap(set.new())
        |> set.to_list
      process.send(reply, convs)
      actor.continue(state)
    }
  }
}

// ── Public API ───────────────────────────────────────────────────────────

/// Supervised child specs the caller adds to a static supervisor. Returns
/// an opaque `Store` in either mode. If `PG_DATABASE_URL` is set in the
/// environment, the pog pool is started and the store talks to Postgres;
/// otherwise it falls back to an in-process actor with the same semantics.
pub fn supervised(
  pg_url: Option(String),
) -> List(supervision.ChildSpecification(Store)) {
  case pg_url {
    option.Some(url) -> {
      let assert Ok(cfg) =
        pog.url_config(process.new_name(prefix: "presence_pog"), url)
      let spec =
        pog.supervised(cfg)
        |> supervision.map_data(fn(connection) {
          Store(mode: Configured(connection))
        })
      [spec]
    }
    option.None -> [
      supervision.worker(fn() { start_mem() })
      |> supervision.map_data(fn(actor_subject) {
        Store(mode: InMemory(actor: actor_subject))
      }),
    ]
  }
}

/// Add (user, conv). Idempotent. Returns the canonical row's id in the
/// configured-mode case, or `""` in memory mode.
pub fn add_member(
  store: Store,
  conv_id conv: ConvId,
  user_id user: UserId,
) -> Result(Nil, String) {
  case store.mode {
    Configured(conn) -> {
      // Upsert conv + user slug rows first so FK constraints and slug
      // round-tripping both work. The `presence_to_uuid()` helper means
      // demo IDs ("conv-1") and real UUIDs both flow through the same
      // column type. The slug columns (`presence_convs.slug` and
      // `presence_users.slug`) keep the original human-readable
      // identifier available so reads can return what the caller
      // expected.
      let conv_sql =
        "insert into presence_convs(id, slug, display_name)
         values (presence_to_uuid($1), $1::text, $1::text)
         on conflict (id) do nothing"
      let _ =
        pog.query(conv_sql)
        |> pog.parameter(pog.text(conv))
        |> pog.execute(conn)

      let user_sql = "select presence_user_upsert($1)"
      let _ =
        pog.query(user_sql)
        |> pog.parameter(pog.text(user))
        |> pog.execute(conn)

      let sql =
        "insert into presence_conv_members
          (conv_id, user_id, role, status, is_soft_deleted)
        values (presence_to_uuid($1), presence_to_uuid($2),
                'member', 'active', false)
        on conflict (conv_id, user_id) where is_soft_deleted = false
        do update set
          status = 'active',
          updated_at = now()"
      pog.query(sql)
      |> pog.parameter(pog.text(conv))
      |> pog.parameter(pog.text(user))
      |> pog.execute(conn)
      |> result.map(fn(_) { Nil })
      |> result.map_error(fn(e) { "pg add_member: " <> string.inspect(e) })
    }
    InMemory(actor) ->
      actor.call(actor, waiting: 500, sending: MemAdd(conv, user, _))
  }
}

pub fn remove_member(
  store: Store,
  conv_id conv: ConvId,
  user_id user: UserId,
) -> Result(Nil, String) {
  case store.mode {
    Configured(conn) -> {
      let sql =
        "update presence_conv_members
          set is_soft_deleted = true,
              left_at = now(),
              updated_at = now()
        where conv_id = presence_to_uuid($1)
          and user_id = presence_to_uuid($2)
          and is_soft_deleted = false"
      pog.query(sql)
      |> pog.parameter(pog.text(conv))
      |> pog.parameter(pog.text(user))
      |> pog.execute(conn)
      |> result.map(fn(_) { Nil })
      |> result.map_error(fn(e) { "pg remove_member: " <> string.inspect(e) })
    }
    InMemory(actor) ->
      actor.call(actor, waiting: 500, sending: MemRemove(conv, user, _))
  }
}

/// Load the conversations a user belongs to. Called by connection actors
/// during their `on_init` so they can register under every `ByConv(_)`
/// group up front.
pub fn convs_of(store: Store, user_id user: UserId) -> List(ConvId) {
  case store.mode {
    Configured(conn) -> {
      // Return slugs (original demo IDs like "conv-1"), not the UUID
      // hashes — the in-memory cache and ws registry use the slugs as
      // their key everywhere downstream.
      let sql =
        "select c.slug
        from presence_conv_members m
        join presence_convs c on c.id = m.conv_id
        where m.user_id = presence_to_uuid($1)
          and m.status = 'active'
          and m.is_soft_deleted = false"
      pog.query(sql)
      |> pog.parameter(pog.text(user))
      |> pog.returning({
        use s <- decode.field(0, decode.string)
        decode.success(s)
      })
      |> pog.execute(conn)
      |> result.map(fn(returned) { returned.rows })
      |> result.unwrap([])
    }
    InMemory(actor) ->
      actor.call(actor, waiting: 500, sending: MemConvsOf(user, _))
  }
}

/// Load the members of a conversation. Called by the conversations actor
/// the first time a local connection mentions a conv we haven't seen yet,
/// to hydrate the in-memory cache for fast member lookups on this node.
pub fn members_of(store: Store, conv_id conv: ConvId) -> List(UserId) {
  case store.mode {
    Configured(conn) -> {
      let sql =
        "select u.slug
        from presence_conv_members m
        join presence_users u on u.id = m.user_id
        where m.conv_id = presence_to_uuid($1)
          and m.status = 'active'
          and m.is_soft_deleted = false"
      pog.query(sql)
      |> pog.parameter(pog.text(conv))
      |> pog.returning({
        use s <- decode.field(0, decode.string)
        decode.success(s)
      })
      |> pog.execute(conn)
      |> result.map(fn(returned) { returned.rows })
      |> result.unwrap([])
    }
    InMemory(actor) ->
      actor.call(actor, waiting: 500, sending: MemMembers(conv, _))
  }
}

/// Best-effort "describe what kind of store this is" for /healthz.
pub fn describe(store: Store) -> String {
  case store.mode {
    Configured(_) -> "postgres (pog pool)"
    InMemory(_) -> "in-memory actor (PG_DATABASE_URL not set)"
  }
}

/// Expose the underlying pog connection for modules that need to issue
/// SQL outside the store's surface (notably `pg_outbox` for tailing the
/// `presence_events` table and `pg_wal` for `pg_logical_slot_get_changes`).
/// Returns `None` when running in the in-memory fallback mode.
pub fn connection(store: Store) -> Option(Connection) {
  case store.mode {
    Configured(conn) -> option.Some(conn)
    InMemory(_) -> option.None
  }
}
