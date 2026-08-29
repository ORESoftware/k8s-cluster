//// Integration: `pg` group membership used by the fanout relay.

import gleam/erlang/process
import gleam/list
import gleamlang_presence_server/fanout
import gleamlang_presence_server/pg_groups
import gleeunit/should

pub fn relay_group_join_is_visible_in_pg_members_test() -> Nil {
  pg_groups.ensure_started()
  let group = fanout.relay_group()
  pg_groups.join(group:)
  let self = process.self()
  pg_groups.members(group:)
  |> list.contains(self)
  |> should.be_true
  pg_groups.remote_members(group:)
  |> should.equal([])
  pg_groups.leave(group:)
}

pub fn two_local_pids_share_a_group_test() -> Nil {
  pg_groups.ensure_started()
  let group = #("e2e-pg-two-pids")
  pg_groups.join(group:)
  let parent = process.self()
  let child =
    process.spawn(fn() {
      pg_groups.join(group:)
      process.sleep(400)
      pg_groups.leave(group:)
    })
  wait_until_member(group, child, 50)
  pg_groups.local_members(group:)
  |> list.contains(parent)
  |> should.be_true
  pg_groups.local_members(group:)
  |> list.contains(child)
  |> should.be_true
  pg_groups.leave(group:)
}

fn wait_until_member(group, pid, attempts: Int) -> Nil {
  case list.contains(pg_groups.local_members(group:), pid) {
    True -> Nil
    False ->
      case attempts {
        0 -> should.fail()
        n -> {
          process.sleep(10)
          wait_until_member(group, pid, n - 1)
        }
      }
  }
}
