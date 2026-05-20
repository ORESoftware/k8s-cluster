//// Integration tests for the conversations actor's fan-out.
////
//// Spins up the full local actor wiring (pg, registry, fanout, in-
//// memory store, conversations actor) and asserts that:
////
////   - `add_member(_, conv, user)` sends a `MembershipChanged(_, AddedToConv(members))`
////     to the user's `ByUser(user)` subscribers, with the conv's full
////     current member list (including the just-added user).
////   - `remove_member(_, conv, user)` sends a `MembershipChanged(_, RemovedFromConv)`
////     to `ByUser(user)` AND a `Kick("removed from conv …")` to
////     `ByUserConv(user, conv)` subscribers.
////   - Subjects subscribed only to `ByConv(_)` (i.e. other members'
////     conv-ws's) do NOT see the membership-changed events — only the
////     `ByUser(_)` channel does.

import gleam/erlang/process
import gleam/int
import gleam/list
import gleam/string
import gleeunit/should
import gleamlang_ws_server/conversations.{type Conversations}
import gleamlang_ws_server/fanout
import gleamlang_ws_server/groups.{
  type ConnGroup, type ConnMsg, AddedToConv, ByConv, ByUser, ByUserConv, Kick,
  MembershipChanged, RemovedFromConv,
}
import gleamlang_ws_server/pg_groups
import gleamlang_ws_server/registry.{type Registry}
import gleamlang_ws_server/store

pub fn add_member_notifies_user_ws_with_full_member_list_test() -> Nil {
  let #(reg, convs) = setup()

  let alice_user_ws = process.new_subject()
  registry.register(reg, ByUser("alice"), alice_user_ws)

  conversations.add_member(convs, conv_id: "c1", user_id: "alice")

  case process.receive(alice_user_ws, 500) {
    Ok(MembershipChanged("c1", AddedToConv(members))) ->
      list.sort(members, string_compare) |> should.equal(["alice"])
    other -> {
      let _ = other
      should.fail()
    }
  }
}

pub fn second_add_includes_first_member_too_test() -> Nil {
  let #(reg, convs) = setup()

  let alice_user_ws = process.new_subject()
  let bob_user_ws = process.new_subject()
  registry.register(reg, ByUser("alice"), alice_user_ws)
  registry.register(reg, ByUser("bob"), bob_user_ws)

  conversations.add_member(convs, conv_id: "c1", user_id: "alice")
  // Drain alice's first notification.
  let _ = process.receive(alice_user_ws, 500)

  conversations.add_member(convs, conv_id: "c1", user_id: "bob")

  // Bob's user-ws sees the full current membership of c1.
  case process.receive(bob_user_ws, 500) {
    Ok(MembershipChanged("c1", AddedToConv(members))) ->
      list.sort(members, string_compare) |> should.equal(["alice", "bob"])
    other -> {
      let _ = other
      should.fail()
    }
  }
}

pub fn remove_member_notifies_user_ws_and_kicks_conv_ws_test() -> Nil {
  let #(reg, convs) = setup()

  let alice_user_ws = process.new_subject()
  let alice_conv_ws = process.new_subject()
  let other_conv_ws = process.new_subject()

  registry.register(reg, ByUser("alice"), alice_user_ws)
  registry.register(reg, ByUserConv("alice", "c1"), alice_conv_ws)
  registry.register(reg, ByConv("c1"), alice_conv_ws)
  // A second conv-ws on c1 that belongs to a DIFFERENT user; it should
  // NOT receive the kick (the kick is targeted via ByUserConv).
  registry.register(reg, ByConv("c1"), other_conv_ws)
  registry.register(reg, ByUserConv("bob", "c1"), other_conv_ws)

  conversations.add_member(convs, conv_id: "c1", user_id: "alice")
  // Drain alice's added-to notification.
  let _ = process.receive(alice_user_ws, 500)

  conversations.remove_member(convs, conv_id: "c1", user_id: "alice")

  // User-ws sees RemovedFromConv.
  case process.receive(alice_user_ws, 500) {
    Ok(MembershipChanged("c1", RemovedFromConv)) -> Nil
    other -> {
      let _ = other
      should.fail()
    }
  }

  // Conv-ws of alice gets kicked.
  case process.receive(alice_conv_ws, 500) {
    Ok(Kick(reason)) ->
      reason |> should.equal("removed from conv c1")
    other -> {
      let _ = other
      should.fail()
    }
  }

  // Other user's conv-ws on the same conv does NOT receive a kick.
  case process.receive(other_conv_ws, 100) {
    Ok(_) -> should.fail()
    Error(_) -> Nil
  }
}

pub fn conv_only_subscribers_dont_get_membership_changed_test() -> Nil {
  // ByConv(_) is the broadcast channel for conv messages; membership
  // changes go to ByUser(_) and ByUserConv(_,_) only. A subject that's
  // ONLY in ByConv should see nothing on add/remove.
  let #(reg, convs) = setup()

  let conv_only = process.new_subject()
  registry.register(reg, ByConv("c1"), conv_only)

  conversations.add_member(convs, conv_id: "c1", user_id: "alice")
  conversations.remove_member(convs, conv_id: "c1", user_id: "alice")

  case process.receive(conv_only, 200) {
    Ok(_) -> should.fail()
    Error(_) -> Nil
  }
}

// ── test helpers ─────────────────────────────────────────────────────────

fn setup() -> #(Registry(ConnMsg, ConnGroup), Conversations) {
  pg_groups.ensure_started()
  // `conversations.start` registers under a globally-stable atom name
  // (`dd_conversations_mesh`) — fine in production where the actor is
  // started once per node, but a no-go when many test cases each spin
  // up their own. Kill any prior owner so the next `actor.named` call
  // can register fresh.
  kill_named("dd_conversations_mesh")

  let tag = int.to_string(unique_int())

  let reg_name = fanout.stable_name("test_conv_reg_" <> tag)
  let reg_started =
    registry.start(reg_name, "test_conv_reg_ets_" <> tag)
    |> should.be_ok
  let reg = reg_started.data

  let fan_name = fanout.stable_name("test_conv_fan_" <> tag)
  let fan_started =
    fanout.start(fan_name, reg)
    |> should.be_ok
  let fan = fan_started.data

  let s = store.start_inmemory() |> should.be_ok

  let convs_started =
    conversations.start(s, reg, fan)
    |> should.be_ok
  #(reg, convs_started.data)
}

/// Test-only helper implemented in `gleamlang_ws_server_ffi.erl`:
/// kill the process registered under the given atom name (if any) and
/// wait until the registration is released. We do this between cases
/// because `conversations.start` registers under a stable atom
/// (`dd_conversations_mesh`) which is unique-per-node in production
/// but collides when many test cases each call `start`.
@external(erlang, "gleamlang_ws_server_ffi", "kill_named")
fn kill_named(name: String) -> Nil

@external(erlang, "erlang", "unique_integer")
fn unique_int_raw(opts: List(a)) -> Int

fn unique_int() -> Int {
  unique_int_raw([])
}

fn string_compare(a: String, b: String) -> _ {
  string.compare(a, b)
}
