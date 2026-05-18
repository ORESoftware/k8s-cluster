//// Thin Gleam wrapper around Erlang's `pg` module (OTP-included since 23,
//// replaced the older `pg2`).
////
//// `pg` is the cross-node half of our registry: each connection PID joins
//// the same group keys that we store in our local ETS registry, but on
//// every node, so peer pods can discover us and we can discover them.
////
//// Reads (`get_members`, `get_local_members`) are internally ETS-backed in
//// `pg` itself — they do not perform any cluster RPC. Replication of joins
//// and leaves happens asynchronously in the background, eventually
//// consistent within a few hundred milliseconds.
////
//// The `pg` scope is a single named process tree that we start under our
//// own supervisor (see `gleamlang_presence_server`). All membership
//// operations refer to that scope.

import gleam/erlang/atom.{type Atom}
import gleam/erlang/process.{type Pid}
import gleam/otp/actor
import gleam/otp/supervision

/// Atom we use as the `pg` scope name throughout the service. Any process
/// that does membership operations must use the same value.
pub fn scope() -> Atom {
  atom.create("presence")
}

@external(erlang, "pg", "start_link")
fn pg_start_link(scope: Atom) -> Result(Pid, anything)

@external(erlang, "pg", "join")
fn pg_join_raw(scope: Atom, group: group, pid: Pid) -> ok

@external(erlang, "pg", "leave")
fn pg_leave_raw(scope: Atom, group: group, pid: Pid) -> ok_or_error

@external(erlang, "pg", "get_members")
fn pg_get_members_raw(scope: Atom, group: group) -> List(Pid)

@external(erlang, "pg", "get_local_members")
fn pg_get_local_members_raw(scope: Atom, group: group) -> List(Pid)

/// Build a child specification that starts the `pg` scope process. Add this
/// to the top of your supervision tree, before anything that calls
/// `join` / `leave` / `get_members`.
///
/// Idempotent if the scope is already running on the node (returns
/// `{:already_started, Pid}` which we treat as success).
pub fn supervised() -> supervision.ChildSpecification(Pid) {
  supervision.worker(fn() {
    case pg_start_link(scope()) {
      Ok(pid) -> Ok(actor.Started(pid:, data: pid))
      Error(_already_started) -> {
        let pid = process.self()
        Ok(actor.Started(pid:, data: pid))
      }
    }
  })
}

/// Idempotent "start the scope if it isn't running yet". Used by tests
/// (and any other code path that needs `join` / `leave` to work but
/// doesn't have a supervisor handy). Safe to call multiple times — `pg`
/// itself returns `{:already_started, Pid}` on subsequent calls.
pub fn ensure_started() -> Nil {
  let _ = pg_start_link(scope())
  Nil
}

pub fn join(group group: group) -> Nil {
  let _ = pg_join_raw(scope(), group, process.self())
  Nil
}

pub fn leave(group group: group) -> Nil {
  let _ = pg_leave_raw(scope(), group, process.self())
  Nil
}

/// All cluster-wide PIDs in `group`, including local ones.
pub fn members(group group: group) -> List(Pid) {
  pg_get_members_raw(scope(), group)
}

/// PIDs in `group` on the calling node only. Microseconds — internal ETS
/// read in `pg`.
pub fn local_members(group group: group) -> List(Pid) {
  pg_get_local_members_raw(scope(), group)
}

/// PIDs in `group` excluding any that live on the calling node. Useful for
/// fanout: send to remote relays, since locals are already covered by the
/// node-local ETS registry.
pub fn remote_members(group group: group) -> List(Pid) {
  let locals = local_members(group)
  members(group)
  |> filter_out(locals)
}

fn filter_out(xs: List(Pid), excluded: List(Pid)) -> List(Pid) {
  case xs {
    [] -> []
    [x, ..rest] ->
      case list_contains(excluded, x) {
        True -> filter_out(rest, excluded)
        False -> [x, ..filter_out(rest, excluded)]
      }
  }
}

fn list_contains(xs: List(Pid), target: Pid) -> Bool {
  case xs {
    [] -> False
    [x, ..rest] ->
      case x == target {
        True -> True
        False -> list_contains(rest, target)
      }
  }
}
