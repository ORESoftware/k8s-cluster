//// Tests for `connection.register_for_scope`.
////
//// Verifies that a UserScope ws registers under exactly `ByUser` (plus
//// the optional `ByUserDevice` shadow), and a ConvScope ws registers
//// under `ByConv` + `ByUserConv` (plus the optional `ByUserDevice`
//// shadow). Critically, neither variant should leak into the *other*
//// scope's group — that's the whole point of the redesign.

import gleam/erlang/process.{type Subject}
import gleam/int
import gleam/list
import gleam/option.{None, Some}
import gleamlang_presence_server/connection.{ConvScope, UserScope}
import gleamlang_presence_server/fanout
import gleamlang_presence_server/groups.{
  type ConnGroup, type ConnMsg, ByConv, ByUser, ByUserConv, ByUserDevice,
}
import gleamlang_presence_server/pg_groups
import gleamlang_presence_server/registry.{type Registry}
import gleeunit/should

pub fn user_scope_without_device_registers_only_by_user_test() -> Nil {
  let reg = setup()
  let subj = process.new_subject()

  connection.register_for_scope(reg, UserScope("alice", None), subj)

  members_include(reg, ByUser("alice"), subj) |> should.be_true
  members_include(reg, ByConv("c1"), subj) |> should.be_false
  members_include(reg, ByUserConv("alice", "c1"), subj) |> should.be_false
  members_include(reg, ByUserDevice("alice", "d1"), subj) |> should.be_false
}

pub fn user_scope_with_device_also_registers_by_user_device_test() -> Nil {
  let reg = setup()
  let subj = process.new_subject()

  connection.register_for_scope(reg, UserScope("alice", Some("d1")), subj)

  members_include(reg, ByUser("alice"), subj) |> should.be_true
  members_include(reg, ByUserDevice("alice", "d1"), subj) |> should.be_true
  members_include(reg, ByConv("c1"), subj) |> should.be_false
  members_include(reg, ByUserConv("alice", "c1"), subj) |> should.be_false
}

pub fn conv_scope_without_device_registers_by_conv_and_user_conv_test() -> Nil {
  let reg = setup()
  let subj = process.new_subject()

  connection.register_for_scope(reg, ConvScope("alice", "c1", None), subj)

  members_include(reg, ByConv("c1"), subj) |> should.be_true
  members_include(reg, ByUserConv("alice", "c1"), subj) |> should.be_true
  members_include(reg, ByUser("alice"), subj) |> should.be_false
  members_include(reg, ByUserDevice("alice", "d1"), subj) |> should.be_false
}

pub fn conv_scope_with_device_also_registers_by_user_device_test() -> Nil {
  let reg = setup()
  let subj = process.new_subject()

  connection.register_for_scope(reg, ConvScope("alice", "c1", Some("d1")), subj)

  members_include(reg, ByConv("c1"), subj) |> should.be_true
  members_include(reg, ByUserConv("alice", "c1"), subj) |> should.be_true
  members_include(reg, ByUserDevice("alice", "d1"), subj) |> should.be_true
  members_include(reg, ByUser("alice"), subj) |> should.be_false
}

pub fn two_devices_of_same_user_both_appear_under_by_user_test() -> Nil {
  let reg = setup()
  let s1 = process.new_subject()
  let s2 = process.new_subject()

  connection.register_for_scope(reg, UserScope("alice", Some("d1")), s1)
  connection.register_for_scope(reg, UserScope("alice", Some("d2")), s2)

  members_include(reg, ByUser("alice"), s1) |> should.be_true
  members_include(reg, ByUser("alice"), s2) |> should.be_true
  members_include(reg, ByUserDevice("alice", "d1"), s1) |> should.be_true
  members_include(reg, ByUserDevice("alice", "d1"), s2) |> should.be_false
  members_include(reg, ByUserDevice("alice", "d2"), s2) |> should.be_true
}

pub fn user_ws_and_conv_ws_share_user_device_group_test() -> Nil {
  // Device-targeted sends should reach BOTH a user-scoped ws and a
  // conv-scoped ws belonging to the same device of the same user.
  let reg = setup()
  let user_ws = process.new_subject()
  let conv_ws = process.new_subject()

  connection.register_for_scope(reg, UserScope("alice", Some("d1")), user_ws)
  connection.register_for_scope(
    reg,
    ConvScope("alice", "c1", Some("d1")),
    conv_ws,
  )

  members_include(reg, ByUserDevice("alice", "d1"), user_ws) |> should.be_true
  members_include(reg, ByUserDevice("alice", "d1"), conv_ws) |> should.be_true
}

// ── test helpers ─────────────────────────────────────────────────────────

/// Start `pg` and a fresh registry. Each test gets a unique Name + ETS
/// table name so the registry actor and its table don't collide with
/// other tests running in the same VM.
fn setup() -> Registry(ConnMsg, ConnGroup) {
  pg_groups.ensure_started()
  let tag = int.to_string(unique_int())
  let name = fanout.stable_name("test_reg_" <> tag)
  let started =
    registry.start(name, "test_reg_ets_" <> tag)
    |> should.be_ok
  started.data
}

fn members_include(
  reg: Registry(ConnMsg, ConnGroup),
  group: ConnGroup,
  subj: Subject(ConnMsg),
) -> Bool {
  registry.members(reg, group)
  |> list.any(fn(s) { s == subj })
}

@external(erlang, "erlang", "unique_integer")
fn unique_int_raw(opts: List(a)) -> Int

fn unique_int() -> Int {
  // monotonic + positive — see erlang:unique_integer/1
  unique_int_raw([])
}
