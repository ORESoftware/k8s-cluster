//// In-process Erlang `pg` membership: join, enumerate, leave, and the
//// local-vs-remote split used by fanout.

import gleam/erlang/process
import gleam/list
import gleamlang_presence_server/pg_groups
import gleeunit/should

pub fn subtract_pids_drops_excluded_test() -> Nil {
  let a = process.self()
  pg_groups.subtract_pids([a], [a])
  |> should.equal([])
}

pub fn join_leave_round_trip_test() -> Nil {
  pg_groups.ensure_started()
  let group = #("unit-pg-join-leave")
  pg_groups.join(group:)
  let self = process.self()
  pg_groups.local_members(group:)
  |> list.contains(self)
  |> should.be_true
  pg_groups.members(group:)
  |> list.contains(self)
  |> should.be_true
  pg_groups.leave(group:)
  pg_groups.local_members(group:)
  |> list.contains(self)
  |> should.be_false
}

pub fn remote_members_empty_on_single_node_test() -> Nil {
  pg_groups.ensure_started()
  let group = #("unit-pg-remote-empty")
  pg_groups.join(group:)
  pg_groups.remote_members(group:)
  |> should.equal([])
  pg_groups.leave(group:)
}

pub fn ensure_started_is_idempotent_test() -> Nil {
  pg_groups.ensure_started()
  pg_groups.ensure_started()
}
