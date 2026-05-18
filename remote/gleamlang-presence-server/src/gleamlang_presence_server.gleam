//// Application entry point. Wires the supervision tree for the presence
//// service:
////
////     main (linked)
////       ├── pg scope        — Erlang `pg` cross-node membership
////       ├── registry        — local ETS group registry  (named, restartable)
////       ├── fanout relay    — per-node broadcast hub    (named)
////       ├── store           — pog pool or in-memory fallback
////       ├── conversations   — durable membership cache
////       ├── pg_listen       — sharded PG LISTEN/NOTIFY  (if PG_DATABASE_URL)
////       ├── nats            — NATS pub/sub transport    (if NATS_URL)
////       └── top supervisor (one_for_one)
////             ├── cluster   — k8s API discovery loop
////             └── mist      — HTTP + websocket server
////
//// Dependencies are linked directly to `main`. If any of them dies, `main`
//// dies, the BEAM exits, and k8s restarts the pod — which is correct
//// since all in-flight connection state goes away anyway. The `cluster`
//// and `mist` subtrees are supervised so transient crashes are handled
//// in-process.
////
//// Layered messaging:
////   1. ETS registry            — local typed sends (microseconds)
////   2. Erlang `pg` + fanout    — within-cluster cross-pod fanout
////   3. PG LISTEN/NOTIFY        — sharded membership change events
////   4. NATS                    — cross-cluster / cross-language pub/sub
////
//// Each layer dedupes against the others so duplicate deliveries collapse.

import gleam/erlang/atom
import gleam/erlang/process
import gleam/int
import gleam/io
import gleam/option
import gleam/otp/static_supervisor as supervisor
import gleam/result
import gleamlang_presence_server/cluster
import gleamlang_presence_server/conversations
import gleamlang_presence_server/fanout
import gleamlang_presence_server/groups
import gleamlang_presence_server/http_server
import gleamlang_presence_server/nats_transport
import gleamlang_presence_server/pg_groups
import gleamlang_presence_server/pg_listen
import gleamlang_presence_server/registry
import gleamlang_presence_server/store

@external(erlang, "gleamlang_presence_server_ffi", "env")
fn env(name: String) -> Result(String, Nil)

@external(erlang, "pg", "start_link")
fn pg_start_link_raw(scope: atom.Atom) -> Result(process.Pid, anything)

pub fn main() {
  let port =
    env("PORT")
    |> result.try(int.parse)
    |> result.unwrap(8081)

  let pg_url = env("PG_DATABASE_URL") |> option.from_result
  let nats_url = env("NATS_URL") |> option.from_result
  let notify_shards =
    env("PRESENCE_NOTIFY_SHARDS")
    |> result.try(int.parse)
    |> result.unwrap(256)

  let _ = case pg_start_link_raw(pg_groups.scope()) {
    Ok(_) -> Nil
    Error(_) -> Nil
  }
  io.println("presence: pg scope ready")

  let reg_name: process.Name(
    registry.Message(groups.ConnMsg, groups.ConnGroup),
  ) = fanout.stable_name("presence_local_registry")
  let assert Ok(reg_started) =
    registry.start(reg_name, "presence_registry_ets")
  let reg = reg_started.data
  io.println("presence: registry started")

  let assert Ok(fanout_started) =
    fanout.start(fanout.relay_name(), reg)
  let fan = fanout_started.data
  io.println("presence: fanout relay started")

  let assert Ok(s) = start_store(pg_url)
  io.println("presence: store ready — " <> store.describe(s))

  let assert Ok(convs_started) = conversations.start(s, reg, fan)
  let convs = convs_started.data
  io.println("presence: conversations actor started")

  // PG LISTEN/NOTIFY (sharded). Optional — only when PG_DATABASE_URL is set.
  case pg_url {
    option.Some(url) -> start_pg_listen(url, convs, notify_shards)
    option.None ->
      io.println("presence: PG_DATABASE_URL unset, skipping LISTEN/NOTIFY")
  }

  // NATS pub/sub. Optional — only when NATS_URL is set. Subscribes to
  // `presence.broadcast.conv.*` so external publishers can fan in.
  let nats_handle = case nats_url {
    option.Some(url) -> start_nats(url, reg, fan)
    option.None -> {
      io.println("presence: NATS_URL unset, skipping NATS transport")
      option.None
    }
  }

  let deps =
    http_server.Deps(
      registry: reg,
      conversations: convs,
      fanout: fan,
      store: s,
      nats: nats_handle,
    )

  let assert Ok(_sup) =
    supervisor.new(supervisor.OneForOne)
    |> supervisor.add(cluster.supervised())
    |> supervisor.add(http_server.supervised(port: port, deps: deps))
    |> supervisor.start()
  io.println("presence: listening on 0.0.0.0:" <> int.to_string(port))

  process.sleep_forever()
}

fn start_store(
  pg_url: option.Option(String),
) -> Result(store.Store, String) {
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
) -> Nil {
  case pg_listen.pgo_config_from_url(url) {
    Error(reason) ->
      io.println("presence: pg_listen disabled (" <> reason <> ")")
    Ok(cfg) -> {
      let on_event = fn(event: pg_listen.Event) -> Nil {
        process.send(convs, conversations.IncomingPgEvent(event))
      }
      case pg_listen.start(cfg, on_event, n_shards) {
        Ok(started) -> {
          conversations.attach_pg_listen(convs, started.data)
          io.println(
            "presence: pg_listen started ("
            <> int.to_string(n_shards)
            <> " shards)",
          )
        }
        Error(_) ->
          io.println("presence: pg_listen failed to start")
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
      nats_transport.subscribe(started.data, "presence.broadcast.conv.>")
      io.println(
        "presence: nats transport started, subscribed to "
        <> "presence.broadcast.conv.>",
      )
      option.Some(started.data)
    }
    Error(_) -> {
      io.println("presence: nats transport failed to start")
      option.None
    }
  }
}
