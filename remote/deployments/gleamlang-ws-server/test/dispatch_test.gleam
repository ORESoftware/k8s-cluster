//// Fan-out dispatch tests for the new `/user/<id>/broadcast` and
//// `/user/<u>/devices/<d>/logout` routes.
////
//// These routes are thin wrappers — the HTTP layer just calls
//// `fanout.broadcast(_, _, group, msg)`. We can therefore validate the
//// dispatch semantics by exercising `fanout.broadcast` directly with
//// the matching `ConnGroup`+`ConnMsg` pair, without spinning up mist
//// or the conversations actor.
////
//// Assertions:
////
////   - `ByUser(u)` broadcast hits every subject registered under
////     `ByUser(u)` AND every subject registered under `ByUserDevice(u,
////     _)` and `ByUserConv(u, _)` (those are "more specific" but the
////     test pins that the user-broadcast does NOT bleed onto them —
////     only `ByUser(u)` subscribers).
////
////   - `ByUserDevice(u, d)` Kick hits only the subjects registered
////     under that exact group; subjects on a different device of the
////     same user OR a different user with the same device id do not
////     receive it.

import gleam/erlang/process
import gleam/int
import gleeunit/should
import gleamlang_ws_server/fanout
import gleamlang_ws_server/groups.{
  type ConnGroup, type ConnMsg, ByUser, ByUserConv, ByUserDevice, Kick, Outbound,
}
import gleamlang_ws_server/pg_groups
import gleamlang_ws_server/registry.{type Registry}

pub fn user_broadcast_only_reaches_by_user_subscribers_test() -> Nil {
  let #(reg, fan) = setup()

  let alice_user_ws = process.new_subject()
  let alice_conv_ws = process.new_subject()
  let alice_device_ws = process.new_subject()
  let bob_user_ws = process.new_subject()

  registry.register(reg, ByUser("alice"), alice_user_ws)
  registry.register(reg, ByUserConv("alice", "c1"), alice_conv_ws)
  registry.register(reg, ByUserDevice("alice", "dev1"), alice_device_ws)
  registry.register(reg, ByUser("bob"), bob_user_ws)
  // `registry.register` posts to the registry actor's mailbox. Wait
  // for it to flush before broadcasting so ETS has the row.
  wait_for_registered(reg, ByUser("alice"))

  fanout.broadcast(fan, reg, ByUser("alice"), Outbound("hello-alice"))

  process.receive(alice_user_ws, 500)
  |> should.equal(Ok(Outbound("hello-alice")))

  // Alice's conv-ws and device-ws are NOT subscribed to ByUser(alice)
  // — they should not receive this frame.
  expect_no_message(alice_conv_ws, 100)
  expect_no_message(alice_device_ws, 100)
  // Bob is a different user.
  expect_no_message(bob_user_ws, 100)
}

pub fn device_kick_reaches_only_that_device_test() -> Nil {
  let #(reg, fan) = setup()

  let alice_dev1_user_ws = process.new_subject()
  let alice_dev1_conv_ws = process.new_subject()
  let alice_dev2_ws = process.new_subject()
  let bob_dev1_ws = process.new_subject()

  // Two ws's of alice/dev1 (user-scoped and conv-scoped) — both
  // register under ByUserDevice("alice", "dev1").
  registry.register(reg, ByUserDevice("alice", "dev1"), alice_dev1_user_ws)
  registry.register(reg, ByUserDevice("alice", "dev1"), alice_dev1_conv_ws)
  // alice's other device.
  registry.register(reg, ByUserDevice("alice", "dev2"), alice_dev2_ws)
  // bob also has a device named "dev1" (device ids aren't globally
  // unique in practice but the group key is keyed on (user, device)
  // so this must NOT collide).
  registry.register(reg, ByUserDevice("bob", "dev1"), bob_dev1_ws)
  wait_for_registered(reg, ByUserDevice("alice", "dev1"))
  wait_for_registered(reg, ByUserDevice("bob", "dev1"))

  fanout.broadcast(
    fan,
    reg,
    ByUserDevice("alice", "dev1"),
    Kick("logout"),
  )

  process.receive(alice_dev1_user_ws, 500)
  |> should.equal(Ok(Kick("logout")))
  process.receive(alice_dev1_conv_ws, 500)
  |> should.equal(Ok(Kick("logout")))

  expect_no_message(alice_dev2_ws, 100)
  expect_no_message(bob_dev1_ws, 100)
}

pub fn user_broadcast_with_no_subscribers_is_a_noop_test() -> Nil {
  let #(reg, fan) = setup()

  // No one is registered under ByUser("ghost").
  fanout.broadcast(fan, reg, ByUser("ghost"), Outbound("nobody-home"))

  // The actor should not crash. We can't really assert "nothing
  // happened" beyond "we can still do more work after", so issue a
  // second broadcast that DOES land and check it lands.
  let alice_user_ws = process.new_subject()
  registry.register(reg, ByUser("alice"), alice_user_ws)
  wait_for_registered(reg, ByUser("alice"))
  fanout.broadcast(fan, reg, ByUser("alice"), Outbound("alive"))
  process.receive(alice_user_ws, 500)
  |> should.equal(Ok(Outbound("alive")))
}

// ── test helpers ─────────────────────────────────────────────────────────

fn setup() -> #(Registry(ConnMsg, ConnGroup), fanout.Fanout) {
  pg_groups.ensure_started()
  let tag = int.to_string(unique_int())

  let reg_name = fanout.stable_name("test_dispatch_reg_" <> tag)
  let reg_started =
    registry.start(reg_name, "test_dispatch_reg_ets_" <> tag)
    |> should.be_ok
  let reg = reg_started.data

  let fan_name = fanout.stable_name("test_dispatch_fan_" <> tag)
  let fan_started =
    fanout.start(fan_name, reg)
    |> should.be_ok
  let fan = fan_started.data

  #(reg, fan)
}

fn expect_no_message(s: process.Subject(a), timeout_ms: Int) -> Nil {
  case process.receive(s, timeout_ms) {
    Ok(_) -> should.fail()
    Error(_) -> Nil
  }
}

/// `registry.register` is an async send to the registry actor — the
/// ETS row only exists after the actor processes the Register message.
/// Spin-poll the registry until the group has at least one member, or
/// time out after ~500ms.
fn wait_for_registered(
  reg: Registry(ConnMsg, ConnGroup),
  group: ConnGroup,
) -> Nil {
  do_wait_for_registered(reg, group, 50)
}

fn do_wait_for_registered(
  reg: Registry(ConnMsg, ConnGroup),
  group: ConnGroup,
  attempts: Int,
) -> Nil {
  case registry.members(reg, group) {
    [_, ..] -> Nil
    [] ->
      case attempts {
        0 -> should.fail()
        n -> {
          process.sleep(10)
          do_wait_for_registered(reg, group, n - 1)
        }
      }
  }
}

@external(erlang, "erlang", "unique_integer")
fn unique_int_raw(opts: List(a)) -> Int

fn unique_int() -> Int {
  unique_int_raw([])
}
