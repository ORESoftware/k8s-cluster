//// Conversations actor.
////
//// Holds an in-memory **subset** of `presence_conv_members`: only the
//// conversations that have at least one live connection on this node, with
//// each conversation's full member list. The full table lives in Postgres
//// (`store.gleam`).
////
//// The actor's responsibilities:
////
////   - Lazy hydration: when a local connection asks "what convs is user X
////     in?" the answer comes from Postgres; the result is cached in
////     `user_convs`. When a join touches a conv we haven't seen, we load
////     its member list from Postgres into `conv_members`.
////   - Source of truth writes: `add_member` / `remove_member` go through
////     Postgres first, then update the cache, then notify the user's live
////     connections cluster-wide:
////       * `MembershipChanged(_, AddedToConv | RemovedFromConv)` is fanned
////         out on `ByUser(user_id)` so every device's user-scoped ws sees
////         the change and can open/close its own conv-scoped ws;
////       * on remove, an additional `Kick(_)` is fanned out on
////         `ByUserConv(user_id, conv_id)` so the matching conv-scoped ws
////         is closed server-side as a defense-in-depth.
////   - Eviction: when the last connection for a user leaves the node, we
////     drop that user's entry from `user_convs` (kept simple here; in
////     production you'd use weak references or periodic GC tied to ETS
////     registry size).
////
//// The cluster-wide step uses `fanout.broadcast(_, group, …)` which
//// delivers to every node that has a matching registration. Peer
//// conversations actors are also gossiped via `pg` so their in-memory
//// caches stay coherent without re-reading Postgres.

import gleam/dict.{type Dict}
import gleam/erlang/atom
import gleam/erlang/process.{type Pid, type Subject}
import gleam/list
import gleam/option
import gleam/otp/actor
import gleam/otp/supervision
import gleam/result
import gleam/set.{type Set}
import gleamlang_presence_server/fanout.{type Fanout}
import gleamlang_presence_server/groups.{
  type ConnGroup, type ConvId, type UserId, AddedToConv, ByUser, ByUserConv,
  Kick, MembershipChanged, RemovedFromConv,
}
import gleamlang_presence_server/pg_groups
import gleamlang_presence_server/pg_listen.{
  type Event as PgEvent, KindAdded, KindRemoved,
}
import gleamlang_presence_server/registry.{type Registry}
import gleamlang_presence_server/store.{type Store}

/// `pg` group every conversations actor joins so peer actors can be
/// reached for membership-change gossip. This is what makes the in-memory
/// store mode work multi-node — when the configured store is Postgres,
/// peers just re-read from the DB instead.
fn mesh_group() -> #(atom.Atom) {
  #(atom.create("conversations_mesh"))
}

pub type Message {
  AddMember(conv_id: ConvId, user_id: UserId)
  RemoveMember(conv_id: ConvId, user_id: UserId)
  ConvsOf(user_id: UserId, reply: Subject(List(ConvId)))
  MembersOf(conv_id: ConvId, reply: Subject(List(UserId)))
  Hydrate(user_id: UserId)
  /// Membership change received from a peer conversations actor over the
  /// Erlang `pg` mesh path. Processed the same way as a local add/remove,
  /// except we do NOT re-write or re-broadcast (would loop).
  PeerEcho(kind: PeerEchoKind, conv_id: ConvId, user_id: UserId)
  /// Membership change observed via the Postgres LISTEN/NOTIFY path.
  /// Carries the trigger's `emitted_at` timestamp so we can dedup against
  /// the parallel pg-mesh delivery.
  IncomingPgEvent(event: PgEvent)
}

pub type PeerEchoKind {
  AddedAt
  RemovedAt
}

pub type Conversations =
  Subject(Message)

/// Stable atom-based Name shared across nodes so peer conversations actors
/// can route gossip envelopes to each other via raw `erlang:send`.
fn mesh_name() -> process.Name(Message) {
  stable_name("dd_conversations_mesh")
}

type State {
  State(
    conv_members: Dict(ConvId, Set(UserId)),
    user_convs: Dict(UserId, Set(ConvId)),
    store: Store,
    registry: Registry(groups.ConnMsg, ConnGroup),
    fanout: Fanout,
    /// Dedup cache for `(conv_id, user_id, kind)` events: last dispatch
    /// timestamp in monotonic ms. If we see the same logical event again
    /// within `dedup_window_ms`, we drop it. Sized by `gc_threshold`.
    last_dispatched_ms: Dict(#(ConvId, UserId, PeerEchoKind), Int),
  )
}

const dedup_window_ms: Int = 500

const gc_threshold: Int = 4096

@external(erlang, "erlang", "monotonic_time")
fn erlang_monotonic_time(unit: atom.Atom) -> Int

fn now_ms() -> Int {
  erlang_monotonic_time(atom.create("millisecond"))
}

pub fn supervised(
  store store: Store,
  registry registry: Registry(groups.ConnMsg, ConnGroup),
  fanout fanout: Fanout,
) -> supervision.ChildSpecification(Conversations) {
  supervision.worker(fn() { start(store, registry, fanout) })
}

pub fn start(
  store: Store,
  registry: Registry(groups.ConnMsg, ConnGroup),
  fanout: Fanout,
) -> Result(actor.Started(Conversations), actor.StartError) {
  let name = mesh_name()
  actor.new_with_initialiser(1000, fn(_self) {
    let _ = pg_groups.join(group: mesh_group())
    let named = process.named_subject(name)
    let selector =
      process.new_selector()
      |> process.select(named)
    actor.initialised(State(
      conv_members: dict.new(),
      user_convs: dict.new(),
      store: store,
      registry: registry,
      fanout: fanout,
      last_dispatched_ms: dict.new(),
    ))
    |> actor.selecting(selector)
    |> actor.returning(named)
    |> Ok
  })
  |> actor.named(name)
  |> actor.on_message(handle)
  |> actor.start()
}

@external(erlang, "erlang", "send")
fn erlang_send(target: Pid, msg: anything) -> anything

@external(erlang, "gleamlang_presence_server_ffi", "stable_name")
fn stable_name(s: String) -> process.Name(msg)

/// Broadcast a membership change to every peer conversations actor in the
/// cluster. The envelope is tagged with the shared stable Name atom of
/// `mesh_name()`, which is the same on every node — peers' selectors
/// (set up in `start`) match on that tag.
fn gossip_peers(kind: PeerEchoKind, conv_id: ConvId, user_id: UserId) -> Nil {
  let peers = pg_groups.remote_members(group: mesh_group())
  list.each(peers, fn(pid: Pid) {
    let _ =
      erlang_send(pid, #(mesh_name(), PeerEcho(kind, conv_id, user_id)))
    Nil
  })
}

/// Add a user to a conversation. Writes Postgres, updates in-memory caches,
/// notifies the user's live connections cluster-wide.
pub fn add_member(
  convs: Conversations,
  conv_id conv_id: ConvId,
  user_id user_id: UserId,
) -> Nil {
  process.send(convs, AddMember(conv_id, user_id))
}

pub fn remove_member(
  convs: Conversations,
  conv_id conv_id: ConvId,
  user_id user_id: UserId,
) -> Nil {
  process.send(convs, RemoveMember(conv_id, user_id))
}

/// List the conversations a user belongs to. The first call for a given
/// user reads from Postgres; subsequent calls are served from cache.
pub fn convs_of(
  convs: Conversations,
  user_id user_id: UserId,
) -> List(ConvId) {
  actor.call(convs, waiting: 500, sending: ConvsOf(user_id, _))
}

pub fn members_of(
  convs: Conversations,
  conv_id conv_id: ConvId,
) -> List(UserId) {
  actor.call(convs, waiting: 500, sending: MembersOf(conv_id, _))
}

/// Force a (possibly redundant) cache hydration for the given user. Called
/// after the connection actor re-registers — the in-memory cache for that
/// user might not exist if the conversations actor was restarted.
pub fn hydrate(
  convs: Conversations,
  user_id user_id: UserId,
) -> Nil {
  process.send(convs, Hydrate(user_id))
}

fn handle(state: State, message: Message) -> actor.Next(State, Message) {
  case message {
    AddMember(conv_id, user_id) -> {
      let written = case store.add_member(state.store, conv_id, user_id) {
        Ok(_) -> True
        Error(_) -> False
      }
      case written {
        False -> actor.continue(state)
        True -> {
          let user_set =
            dict.get(state.user_convs, user_id)
            |> result.unwrap(set.new())
            |> set.insert(conv_id)
          let conv_set =
            case dict.get(state.conv_members, conv_id) {
              Ok(s) -> s
              Error(_) ->
                store.members_of(state.store, conv_id) |> set.from_list
            }
            |> set.insert(user_id)
          let new_state =
            State(
              ..state,
              conv_members: dict.insert(state.conv_members, conv_id, conv_set),
              user_convs: dict.insert(state.user_convs, user_id, user_set),
            )
          // Embed the conv's full current member list so the user-ws's
          // client can decide what to do without a follow-up GET on
          // /conv/<id>/members.
          let members = set.to_list(conv_set)
          fanout.broadcast(
            state.fanout,
            state.registry,
            ByUser(user_id),
            MembershipChanged(conv_id, AddedToConv(members)),
          )
          gossip_peers(AddedAt, conv_id, user_id)
          actor.continue(new_state)
        }
      }
    }

    RemoveMember(conv_id, user_id) -> {
      let written = case store.remove_member(state.store, conv_id, user_id) {
        Ok(_) -> True
        Error(_) -> False
      }
      case written {
        False -> actor.continue(state)
        True -> {
          let user_set =
            dict.get(state.user_convs, user_id)
            |> result.unwrap(set.new())
            |> set.delete(conv_id)
          let conv_set =
            dict.get(state.conv_members, conv_id)
            |> result.unwrap(set.new())
            |> set.delete(user_id)
          let user_convs = case set.is_empty(user_set) {
            True -> dict.delete(state.user_convs, user_id)
            False -> dict.insert(state.user_convs, user_id, user_set)
          }
          let conv_members = case set.is_empty(conv_set) {
            True -> dict.delete(state.conv_members, conv_id)
            False -> dict.insert(state.conv_members, conv_id, conv_set)
          }
          fanout.broadcast(
            state.fanout,
            state.registry,
            ByUser(user_id),
            MembershipChanged(conv_id, RemovedFromConv),
          )
          // Defense-in-depth: close that user's conv-scoped ws's for
          // this conv cluster-wide. The client should also close them
          // on its own after seeing the MembershipChanged above, but
          // we don't trust the client.
          fanout.broadcast(
            state.fanout,
            state.registry,
            ByUserConv(user_id, conv_id),
            Kick("removed from conv " <> conv_id),
          )
          gossip_peers(RemovedAt, conv_id, user_id)
          actor.continue(State(
            ..state,
            conv_members: conv_members,
            user_convs: user_convs,
          ))
        }
      }
    }

    ConvsOf(user_id, reply) -> {
      // Cache miss → load from Postgres.
      let #(convs, new_state) = case dict.get(state.user_convs, user_id) {
        Ok(s) -> #(set.to_list(s), state)
        Error(_) -> {
          let from_pg = store.convs_of(state.store, user_id)
          #(
            from_pg,
            State(
              ..state,
              user_convs: dict.insert(
                state.user_convs,
                user_id,
                set.from_list(from_pg),
              ),
            ),
          )
        }
      }
      process.send(reply, convs)
      actor.continue(new_state)
    }

    MembersOf(conv_id, reply) -> {
      let #(users, new_state) = case dict.get(state.conv_members, conv_id) {
        Ok(s) -> #(set.to_list(s), state)
        Error(_) -> {
          let from_pg = store.members_of(state.store, conv_id)
          #(
            from_pg,
            State(
              ..state,
              conv_members: dict.insert(
                state.conv_members,
                conv_id,
                set.from_list(from_pg),
              ),
            ),
          )
        }
      }
      process.send(reply, users)
      actor.continue(new_state)
    }

    PeerEcho(kind, conv_id, user_id) -> {
      case should_dispatch(state, conv_id, user_id, kind) {
        False -> actor.continue(state)
        True ->
          dispatch_peer_change(
            record_dispatch(state, conv_id, user_id, kind),
            kind,
            conv_id,
            user_id,
          )
      }
    }

    IncomingPgEvent(event) -> {
      // Translate the LISTEN payload into the same PeerEcho semantics.
      // Dedup against the parallel pg-mesh path via `last_dispatched_ms`.
      case pg_listen.semantic_kind(event) {
        option.None -> actor.continue(state)
        option.Some(pg_kind) -> {
          let kind = case pg_kind {
            KindAdded -> AddedAt
            KindRemoved -> RemovedAt
          }
          case should_dispatch(state, event.conv_id, event.user_id, kind) {
            False -> actor.continue(state)
            True ->
              dispatch_peer_change(
                record_dispatch(state, event.conv_id, event.user_id, kind),
                kind,
                event.conv_id,
                event.user_id,
              )
          }
        }
      }
    }

    Hydrate(user_id) -> {
      let from_pg = store.convs_of(state.store, user_id)
      let conv_members =
        list.fold(from_pg, state.conv_members, fn(acc, conv_id) {
          case dict.has_key(acc, conv_id) {
            True -> acc
            False ->
              dict.insert(
                acc,
                conv_id,
                set.from_list(store.members_of(state.store, conv_id)),
              )
          }
        })
      actor.continue(State(
        ..state,
        user_convs: dict.insert(
          state.user_convs,
          user_id,
          set.from_list(from_pg),
        ),
        conv_members: conv_members,
      ))
    }
  }
}

fn should_dispatch(
  state: State,
  conv_id: ConvId,
  user_id: UserId,
  kind: PeerEchoKind,
) -> Bool {
  case dict.get(state.last_dispatched_ms, #(conv_id, user_id, kind)) {
    Ok(t) -> now_ms() - t > dedup_window_ms
    Error(_) -> True
  }
}

fn record_dispatch(
  state: State,
  conv_id: ConvId,
  user_id: UserId,
  kind: PeerEchoKind,
) -> State {
  let updated =
    dict.insert(state.last_dispatched_ms, #(conv_id, user_id, kind), now_ms())
  // Cheap periodic GC: when the cache crosses the threshold, drop any
  // entries older than the dedup window times 8 (so we don't churn).
  case dict.size(updated) > gc_threshold {
    False -> State(..state, last_dispatched_ms: updated)
    True -> {
      let cutoff = now_ms() - { dedup_window_ms * 8 }
      let gced =
        dict.filter(updated, fn(_key, t) { t > cutoff })
      State(..state, last_dispatched_ms: gced)
    }
  }
}

fn dispatch_peer_change(
  state: State,
  kind: PeerEchoKind,
  conv_id: ConvId,
  user_id: UserId,
) -> actor.Next(State, Message) {
  let #(new_state, change) = case kind {
    AddedAt -> {
      let user_set =
        dict.get(state.user_convs, user_id)
        |> result.unwrap(set.new())
        |> set.insert(conv_id)
      let conv_set =
        dict.get(state.conv_members, conv_id)
        |> result.unwrap(set.new())
        |> set.insert(user_id)
      let ns =
        State(
          ..state,
          user_convs: dict.insert(state.user_convs, user_id, user_set),
          conv_members: dict.insert(state.conv_members, conv_id, conv_set),
        )
      #(ns, AddedToConv(set.to_list(conv_set)))
    }
    RemovedAt -> {
      let user_set =
        dict.get(state.user_convs, user_id)
        |> result.unwrap(set.new())
        |> set.delete(conv_id)
      let conv_set =
        dict.get(state.conv_members, conv_id)
        |> result.unwrap(set.new())
        |> set.delete(user_id)
      let user_convs = case set.is_empty(user_set) {
        True -> dict.delete(state.user_convs, user_id)
        False -> dict.insert(state.user_convs, user_id, user_set)
      }
      let conv_members = case set.is_empty(conv_set) {
        True -> dict.delete(state.conv_members, conv_id)
        False -> dict.insert(state.conv_members, conv_id, conv_set)
      }
      let ns =
        State(..state, user_convs: user_convs, conv_members: conv_members)
      #(ns, RemovedFromConv)
    }
  }
  // Dispatch the membership change to LOCAL user-scoped ws's of this
  // user. The originating node already did the cluster-wide dispatch
  // via fanout.broadcast, so this is local-only.
  registry.dispatch_group(state.registry, ByUser(user_id), fn(subj) {
    process.send(subj, MembershipChanged(conv_id, change))
    Nil
  })
  // On remove, also kick this user's local conv-scoped ws's for this
  // conv — defense-in-depth on top of the cluster-wide kick that the
  // originating node already did.
  case kind {
    RemovedAt ->
      registry.dispatch_group(
        state.registry,
        ByUserConv(user_id, conv_id),
        fn(subj) {
          process.send(subj, Kick("removed from conv " <> conv_id))
          Nil
        },
      )
    AddedAt -> Nil
  }
  actor.continue(new_state)
}
