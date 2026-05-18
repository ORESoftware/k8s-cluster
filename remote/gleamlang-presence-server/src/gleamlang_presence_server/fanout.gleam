//// Per-node fanout relay.
////
//// On every node, exactly one of these processes runs under a stable name
//// (`presence_fanout`). It is registered both:
////
////   1. As a Gleam `Name(FanoutMsg)` — so the local broadcast code can use
////      typed sends.
////   2. As a globally-named Erlang registered process via `pg:join` under
////      a well-known group `(:relay,)` — so peer nodes can find it by
////      looking at `pg:get_members` of that group.
////
//// When the relay receives a `Forward(group, msg)` envelope it does the
//// local ETS dispatch — i.e., it asks its node-local registry "who is in
//// `group` here?" and `process.send`s `msg` to each. This is the standard
//// Phoenix.PubSub pattern: one cross-node message per peer node, not one
//// per subscriber.

import gleam/erlang/atom
import gleam/erlang/process.{type Name, type Pid}
import gleam/list
import gleam/otp/actor
import gleam/otp/supervision
import gleamlang_presence_server/groups.{type ConnGroup, type ConnMsg}
import gleamlang_presence_server/pg_groups
import gleamlang_presence_server/registry.{type Registry}

/// Build a stable `Name(msg)` whose underlying atom is the same on every
/// node. Required for cross-node sends to typed subjects.
@external(erlang, "gleamlang_presence_server_ffi", "stable_name")
pub fn stable_name(s: String) -> Name(msg)

/// Stable Name shared by every node's fanout relay so cross-node sends
/// route to the right process.
pub fn relay_name() -> Name(FanoutMsg) {
  stable_name("dd_presence_fanout_relay")
}

pub type FanoutMsg {
  Forward(group: ConnGroup, msg: ConnMsg)
}

pub type Fanout {
  Fanout(name: Name(FanoutMsg))
}

type State {
  State(registry: Registry(ConnMsg, ConnGroup))
}

/// Group key used in `pg` to discover every node's fanout relay. The local
/// broadcast code does `pg_groups.members(relay_group())` to enumerate
/// remote relays.
pub fn relay_group() -> #(atom.Atom) {
  #(atom.create("relay"))
}

pub fn supervised(
  registry registry: Registry(ConnMsg, ConnGroup),
) -> supervision.ChildSpecification(Fanout) {
  supervision.worker(fn() { start(relay_name(), registry) })
}

pub fn start(
  name: Name(FanoutMsg),
  reg: Registry(ConnMsg, ConnGroup),
) -> Result(actor.Started(Fanout), actor.StartError) {
  actor.new_with_initialiser(1000, fn(_self) {
    // Announce this relay to the cluster via `pg`. From now on peer nodes
    // calling `pg_groups.members(relay_group())` will see this PID.
    let _ = pg_groups.join(group: relay_group())

    actor.initialised(State(registry: reg))
    |> actor.returning(Fanout(name: name))
    |> Ok
  })
  |> actor.named(name)
  |> actor.on_message(handle)
  |> actor.start()
}

fn handle(state: State, message: FanoutMsg) -> actor.Next(State, FanoutMsg) {
  case message {
    Forward(group, msg) -> {
      registry.dispatch_group(state.registry, group, fn(subj) {
        process.send(subj, msg)
        Nil
      })
      actor.continue(state)
    }
  }
}

/// Broadcast `msg` to every connection in `group`, on every node in the
/// cluster.
///
///   1. Local dispatch via ETS — typed sends, microseconds.
///   2. Remote dispatch: ask `pg` for cluster-wide PIDs of `relay_group()`,
///      filter out our own PID, send each remote relay a `Forward`
///      envelope. Each remote relay then does its own local ETS dispatch.
///
/// The number of cross-node sends is O(peer nodes), not O(remote subscribers).
pub fn broadcast(
  fanout: Fanout,
  registry: Registry(ConnMsg, ConnGroup),
  group: ConnGroup,
  msg: ConnMsg,
) -> Nil {
  registry.dispatch_group(registry, group, fn(subj) {
    process.send(subj, msg)
    Nil
  })

  let remote_relays = pg_groups.remote_members(group: relay_group())

  list.each(remote_relays, fn(relay_pid: Pid) {
    // Send as #(stable_name_atom, msg) — the remote relay's selector
    // (`process.select(named_subject(relay_name()))`) matches on that tag.
    let _ = erlang_send(relay_pid, #(name_tag(fanout), Forward(group, msg)))
    Nil
  })
  Nil
}

@external(erlang, "erlang", "send")
fn erlang_send(target: Pid, msg: anything) -> anything

fn name_tag(fanout: Fanout) -> Name(FanoutMsg) {
  fanout.name
}
