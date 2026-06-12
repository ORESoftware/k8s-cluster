//// Application entry point for the merged ws-server.
////
//// Two subsystems live in one BEAM node, side by side:
////
////   1. The legacy *broadcaster* ticker — a 2-second tick stream
////      with JSON-id dedup, served on `/worker-ws/<secret>`. Inbound
////      `/broadcast` HTTP fan-outs from a NATS-bridge sidecar publish
////      into the same broadcaster. `/metrics` reports its counters in
////      Prometheus format.
////
////   2. The new *presence cluster* — a multi-node BEAM cluster that
////      uses Erlang `pg` for cross-node membership, an ETS subject
////      registry per pod for typed local sends, a fanout relay per
////      node, a Postgres-backed conversations actor with optional
////      sharded LISTEN/NOTIFY + wal2json CDC + outbox tail, plus an
////      optional native NATS transport. Served on `/ws?user=…` and
////      friends.
////
//// Supervision tree:
////
////     main (linked)
////       ├── pg scope        — Erlang `pg` cross-node membership
////       ├── registry        — local ETS group registry (named, restartable)
////       ├── fanout relay    — per-node broadcast hub (named)
////       ├── store           — pog pool or in-memory fallback
////       ├── conversations   — durable membership cache
////       ├── pg_listen       — sharded PG LISTEN/NOTIFY (if PG_DATABASE_URL)
////       ├── nats            — NATS pub/sub transport (if NATS_URL)
////       └── top supervisor (one_for_one)
////             ├── broadcaster — legacy tick stream (named)
////             ├── cluster   — k8s API discovery loop
////             └── mist      — HTTP + websocket server
////
//// Dependencies before the supervisor are linked directly to `main`. If
//// any of them dies, `main` dies, the BEAM exits, and k8s restarts the
//// pod — which is correct since all in-flight connection state goes
//// away anyway. The `broadcaster`, `cluster`, and `mist` subtrees are
//// supervised so transient crashes are handled in-process.

import dd_cli_config_client
import dd_otel_client
import dd_runtime_config_client
import gleam/erlang/atom
import gleam/erlang/process
import gleam/int
import gleam/io
import gleam/option
import gleam/otp/actor
import gleam/otp/static_supervisor as supervisor
import gleam/otp/supervision
import gleam/result
import gleam/string
import gleamlang_ws_server/broadcaster
import gleamlang_ws_server/cluster
import gleamlang_ws_server/conversations
import gleamlang_ws_server/fanout
import gleamlang_ws_server/groups
import gleamlang_ws_server/http_server
import gleamlang_ws_server/nats_transport
import gleamlang_ws_server/pg_contract
import gleamlang_ws_server/pg_groups
import gleamlang_ws_server/pg_listen
import gleamlang_ws_server/pg_outbox
import gleamlang_ws_server/pg_wal
import gleamlang_ws_server/registry
import gleamlang_ws_server/store
import pog

const broadcaster_tick_interval_ms = 2000

@external(erlang, "gleamlang_ws_server_ffi", "env")
fn env(name: String) -> Result(String, Nil)

@external(erlang, "pg", "start_link")
fn pg_start_link_raw(scope: atom.Atom) -> Result(process.Pid, anything)

pub fn main() {
  let _ = dd_cli_config_client.load_once()
  // Start the OpenTelemetry SDK + OTLP exporter before the HTTP supervisor.
  let _ = dd_otel_client.init("dd-gleamlang-ws-server")
  let port =
    env("PORT")
    |> result.try(int.parse)
    |> result.unwrap(8081)

  let pg_url = env("PG_DATABASE_URL") |> option.from_result
  let nats_url = env("NATS_URL") |> option.from_result
  let notify_shards = positive_int_env("PRESENCE_NOTIFY_SHARDS", 256)

  let _ = pg_contract.app_config_table()

  let _ = case pg_start_link_raw(pg_groups.scope()) {
    Ok(_) -> Nil
    Error(_) -> Nil
  }
  io.println("ws-server: pg scope ready")

  let reg_name: process.Name(registry.Message(groups.ConnMsg, groups.ConnGroup)) =
    fanout.stable_name("presence_local_registry")
  let assert Ok(reg_started) = registry.start(reg_name, "presence_registry_ets")
  let reg = reg_started.data
  io.println("ws-server: registry started")

  let assert Ok(fanout_started) = fanout.start(fanout.relay_name(), reg)
  let fan = fanout_started.data
  io.println("ws-server: fanout relay started")

  let assert Ok(s) = start_store(pg_url)
  io.println("ws-server: store ready — " <> store.describe(s))

  let assert Ok(convs_started) = conversations.start(s, reg, fan)
  let convs = convs_started.data
  io.println("ws-server: conversations actor started")

  let outbox_handle = case store.connection(s) {
    option.Some(conn) -> start_pg_outbox(conn, convs)
    option.None -> {
      io.println("ws-server: store has no pog connection, skipping pg_outbox")
      option.None
    }
  }

  case store.connection(s), bool_env("PRESENCE_WAL_ENABLED", False) {
    option.Some(conn), True -> start_pg_wal(conn, convs)
    option.Some(_conn), False ->
      io.println(
        "ws-server: pg_wal disabled (set PRESENCE_WAL_ENABLED=true to enable)",
      )
    option.None, _ ->
      io.println("ws-server: store has no pog connection, skipping pg_wal")
  }

  case pg_url {
    option.Some(url) ->
      start_pg_listen(url, convs, notify_shards, outbox_handle)
    option.None ->
      io.println("ws-server: PG_DATABASE_URL unset, skipping LISTEN/NOTIFY")
  }

  let nats_handle = case nats_url {
    option.Some(url) -> start_nats(url, reg, fan)
    option.None -> {
      io.println("ws-server: NATS_URL unset, skipping NATS transport")
      option.None
    }
  }

  let broker_name = process.new_name(prefix: "ws_server_broadcaster")

  let deps =
    http_server.Deps(
      registry: reg,
      conversations: convs,
      fanout: fan,
      store: s,
      nats: nats_handle,
      broadcaster: broker_name,
    )

  let assert Ok(_sup) =
    supervisor.new(supervisor.OneForOne)
    |> supervisor.add(
      supervision.worker(fn() {
        broadcaster.start(
          named_as: broker_name,
          interval_ms: broadcaster_tick_interval_ms,
        )
      }),
    )
    |> supervisor.add(cluster.supervised())
    |> supervisor.add(http_server.supervised(port: port, deps: deps))
    |> supervisor.start()
  io.println("ws-server: listening on 0.0.0.0:" <> int.to_string(port))

  let _ = dd_runtime_config_client.start_registration_loop()

  process.sleep_forever()
}

fn start_store(pg_url: option.Option(String)) -> Result(store.Store, String) {
  case pg_url {
    option.Some(url) ->
      store.start_postgres(url)
      |> result.map_error(fn(_) {
        "Failed to start pog pool with PG_DATABASE_URL"
      })
    option.None ->
      store.start_inmemory()
      |> result.map_error(fn(_) { "Failed to start in-memory store" })
  }
}

fn start_pg_listen(
  url: String,
  convs: conversations.Conversations,
  n_shards: Int,
  outbox: option.Option(pg_outbox.PgOutbox),
) -> Nil {
  case pg_listen.pgo_config_from_url(url) {
    Error(reason) ->
      io.println("ws-server: pg_listen disabled (" <> reason <> ")")
    Ok(cfg) -> {
      let on_event = fn(event: pg_listen.Event) -> Nil {
        process.send(convs, conversations.IncomingPgEvent(event))
        case outbox {
          option.Some(ob) -> pg_outbox.wake(ob, high_water: event.seq)
          option.None -> Nil
        }
      }
      case pg_listen.start(cfg, on_event, n_shards) {
        Ok(started) -> {
          conversations.attach_pg_listen(convs, started.data)
          io.println(
            "ws-server: pg_listen started ("
            <> int.to_string(n_shards)
            <> " shards)",
          )
        }
        Error(_) -> io.println("ws-server: pg_listen failed to start")
      }
    }
  }
}

fn start_pg_wal(
  conn: pog.Connection,
  convs: conversations.Conversations,
) -> Nil {
  let n_shards = positive_int_env("PRESENCE_NOTIFY_SHARDS", 256)
  let tick_ms = positive_int_env("PRESENCE_WAL_TICK_MS", 1000)
  let on_event = fn(event: pg_listen.Event) -> Nil {
    process.send(convs, conversations.IncomingPgEvent(event))
  }
  case pg_wal.start(conn, on_event, n_shards, tick_ms) {
    Ok(_started) ->
      io.println(
        "ws-server: pg_wal started (tick=" <> int.to_string(tick_ms) <> "ms)",
      )
    Error(actor.InitFailed(reason)) ->
      io.println("ws-server: pg_wal disabled — " <> reason)
    Error(_) -> io.println("ws-server: pg_wal failed to start (unknown reason)")
  }
}

fn start_pg_outbox(
  conn: pog.Connection,
  convs: conversations.Conversations,
) -> option.Option(pg_outbox.PgOutbox) {
  let n_shards = positive_int_env("PRESENCE_NOTIFY_SHARDS", 256)
  let tick_ms = positive_int_env("PRESENCE_OUTBOX_TICK_MS", 5000)
  let on_event = fn(event: pg_listen.Event) -> Nil {
    process.send(convs, conversations.IncomingPgEvent(event))
  }
  case pg_outbox.start(conn, on_event, n_shards, tick_ms) {
    Ok(started) -> {
      conversations.attach_pg_outbox(convs, started.data)
      io.println(
        "ws-server: pg_outbox started (tick="
        <> int.to_string(tick_ms)
        <> "ms, shards="
        <> int.to_string(n_shards)
        <> ")",
      )
      option.Some(started.data)
    }
    Error(_) -> {
      io.println("ws-server: pg_outbox failed to start")
      option.None
    }
  }
}

fn positive_int_env(name: String, fallback: Int) -> Int {
  case env(name) |> result.try(int.parse) {
    Ok(n) -> {
      case n > 0 {
        True -> n
        False -> fallback
      }
    }
    Error(_) -> fallback
  }
}

fn bool_env(name: String, fallback: Bool) -> Bool {
  case env(name) {
    Error(_) -> fallback
    Ok(raw) -> {
      case string.lowercase(raw) {
        "1" -> True
        "true" -> True
        "yes" -> True
        "on" -> True
        "0" -> False
        "false" -> False
        "no" -> False
        "off" -> False
        _ -> fallback
      }
    }
  }
}

fn start_nats(
  url: String,
  reg: registry.Registry(groups.ConnMsg, groups.ConnGroup),
  fan: fanout.Fanout,
) -> option.Option(nats_transport.Nats) {
  let on_msg = fn(inbound: nats_transport.Inbound) -> Nil {
    nats_transport.dispatch_inbound_default(inbound, reg, fan)
  }
  case nats_transport.start(url, on_msg) {
    Ok(started) -> {
      nats_transport.subscribe(
        started.data,
        nats_transport.broadcast_conv_wildcard,
      )
      io.println(
        "ws-server: nats transport started, subscribed to "
        <> nats_transport.broadcast_conv_wildcard,
      )
      option.Some(started.data)
    }
    Error(_) -> {
      io.println("ws-server: nats transport failed to start")
      option.None
    }
  }
}
