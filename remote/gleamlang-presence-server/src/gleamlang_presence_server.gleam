//// Application entry point. Wires the supervision tree for the presence
//// service:
////
////     main (linked)
////       ├── pg scope        — Erlang `pg` cross-node membership
////       ├── registry        — local ETS group registry  (named, restartable)
////       ├── fanout relay    — per-node broadcast hub    (named)
////       ├── store           — pog pool or in-memory fallback
////       ├── conversations   — durable membership cache
////       └── top supervisor (one_for_one)
////             ├── cluster   — k8s API discovery loop
////             └── mist      — HTTP + websocket server
////
//// Dependencies are linked directly to `main`. If any of them dies, `main`
//// dies, the BEAM exits, and k8s restarts the pod — which is correct
//// since all in-flight connection state goes away anyway. The `cluster`
//// and `mist` subtrees are supervised so transient crashes are handled
//// in-process.

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
import gleamlang_presence_server/pg_groups
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

  let deps =
    http_server.Deps(
      registry: reg,
      conversations: convs,
      fanout: fan,
      store: s,
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
