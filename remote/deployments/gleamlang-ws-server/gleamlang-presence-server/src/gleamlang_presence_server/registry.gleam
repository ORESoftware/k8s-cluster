//// Local ETS-backed group registry.
////
//// One row per (group, pid, subject). Reads (`members`, `dispatch_group`)
//// hit ETS directly from the caller's process — no actor hop, microsecond
//// concurrent. Mutations (register, unregister) go through the named actor
//// which owns the table and the per-pid monitors so the death cleanup is
//// race-free.
////
//// The actor has a stable `Name` so connections can monitor it and
//// re-register if the supervisor restarts it. This is the (a) half of the
//// re-registration design from the chat with the user.

import gleam/dict.{type Dict}
import gleam/erlang/atom.{type Atom}
import gleam/erlang/process.{type Name, type Pid, type Subject, ProcessDown}
import gleam/list
import gleam/otp/actor
import gleam/otp/supervision
import gleam/result

pub type Table

pub type Registry(msg, group) {
  Registry(name: Name(Message(msg, group)), table: Table)
}

pub opaque type Message(msg, group) {
  Register(group: group, subject: Subject(msg))
  Unregister(group: group, subject: Subject(msg))
  Down(pid: Pid)
}

type State(msg, group) {
  State(
    table: Table,
    pid_rows: Dict(Pid, List(#(group, Subject(msg)))),
    monitored: Dict(Pid, process.Monitor),
  )
}

@external(erlang, "ets", "new")
fn ets_new(name: Atom, options: List(EtsOption)) -> Table

@external(erlang, "ets", "insert")
fn ets_insert(table: Table, row: row) -> Bool

@external(erlang, "ets", "lookup")
fn ets_lookup(table: Table, key: key) -> List(row)

@external(erlang, "ets", "delete_object")
fn ets_delete_object(table: Table, row: row) -> Bool

type EtsOption {
  NamedTable
  Protected
  Bag
  ReadConcurrency(Bool)
}

/// Supervised child spec. The supervisor restarts this on crash with
/// strategy `OneForOne`. After restart the table is empty; connections
/// re-register themselves when they see the monitor `DOWN`.
pub fn supervised(
  name name: Name(Message(msg, group)),
  table_name table_name: String,
) -> supervision.ChildSpecification(Registry(msg, group)) {
  supervision.worker(fn() { start(name, table_name) })
}

pub fn start(
  name: Name(Message(msg, group)),
  table_name: String,
) -> Result(actor.Started(Registry(msg, group)), actor.StartError) {
  actor.new_with_initialiser(1000, fn(_self) {
    let table =
      ets_new(atom.create(table_name), [
        NamedTable,
        Protected,
        Bag,
        ReadConcurrency(True),
      ])
    let me = process.named_subject(name)
    let selector =
      process.new_selector()
      |> process.select(me)
      |> process.select_monitors(fn(down) {
        case down {
          ProcessDown(pid:, ..) -> Down(pid)
          _ -> Down(process.self())
        }
      })
    actor.initialised(State(
      table: table,
      pid_rows: dict.new(),
      monitored: dict.new(),
    ))
    |> actor.selecting(selector)
    |> actor.returning(Registry(name: name, table: table))
    |> Ok
  })
  |> actor.named(name)
  |> actor.on_message(handle)
  |> actor.start()
}

pub fn register(
  registry: Registry(msg, group),
  group group: group,
  subject subject: Subject(msg),
) -> Nil {
  process.send(process.named_subject(registry.name), Register(group, subject))
}

pub fn unregister(
  registry: Registry(msg, group),
  group group: group,
  subject subject: Subject(msg),
) -> Nil {
  process.send(process.named_subject(registry.name), Unregister(group, subject))
}

pub fn members(
  registry: Registry(msg, group),
  group group: group,
) -> List(Subject(msg)) {
  ets_lookup(registry.table, group)
  |> list.map(fn(row: #(group, Pid, Subject(msg))) { row.2 })
}

pub fn dispatch_group(
  registry: Registry(msg, group),
  group group: group,
  callback callback: fn(Subject(msg)) -> Nil,
) -> Nil {
  members(registry, group)
  |> list.each(callback)
}

/// Look up the registry actor's current pid by name. Used by connections to
/// set up a monitor; if the registry restarts, the connection re-registers.
pub fn whereis(registry: Registry(msg, group)) -> Result(Pid, Nil) {
  process.subject_owner(process.named_subject(registry.name))
}

fn handle(
  state: State(msg, group),
  message: Message(msg, group),
) -> actor.Next(State(msg, group), Message(msg, group)) {
  case message {
    Register(group, subject) -> {
      case process.subject_owner(subject) {
        Ok(pid) -> {
          let _ = ets_insert(state.table, #(group, pid, subject))
          let prev = dict.get(state.pid_rows, pid) |> result.unwrap([])
          let pid_rows =
            dict.insert(state.pid_rows, pid, [#(group, subject), ..prev])
          let monitored = case dict.has_key(state.monitored, pid) {
            True -> state.monitored
            False -> dict.insert(state.monitored, pid, process.monitor(pid))
          }
          actor.continue(State(
            table: state.table,
            pid_rows: pid_rows,
            monitored: monitored,
          ))
        }
        Error(_) -> actor.continue(state)
      }
    }
    Unregister(group, subject) -> {
      case process.subject_owner(subject) {
        Ok(pid) -> {
          let _ = ets_delete_object(state.table, #(group, pid, subject))
          let pid_rows = case dict.get(state.pid_rows, pid) {
            Ok(rows) -> {
              let remaining =
                list.filter(rows, fn(r) { !{ r.0 == group && r.1 == subject } })
              case remaining {
                [] -> dict.delete(state.pid_rows, pid)
                _ -> dict.insert(state.pid_rows, pid, remaining)
              }
            }
            Error(_) -> state.pid_rows
          }
          actor.continue(State(..state, pid_rows: pid_rows))
        }
        Error(_) -> actor.continue(state)
      }
    }
    Down(pid) -> {
      case dict.get(state.pid_rows, pid) {
        Ok(rows) ->
          list.each(rows, fn(row) {
            let #(group, subject) = row
            let _ = ets_delete_object(state.table, #(group, pid, subject))
            Nil
          })
        Error(_) -> Nil
      }
      actor.continue(State(
        table: state.table,
        pid_rows: dict.delete(state.pid_rows, pid),
        monitored: dict.delete(state.monitored, pid),
      ))
    }
  }
}
