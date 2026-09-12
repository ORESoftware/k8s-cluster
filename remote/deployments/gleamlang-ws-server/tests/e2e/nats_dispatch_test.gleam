//// Integration: inbound NATS dispatch vs BEAM `pg` dedup.

import gleam/bit_array
import gleam/erlang/process
import gleam/int
import gleam/option.{None, Some}
import gleamlang_ws_server/fanout
import gleamlang_ws_server/groups.{ByConv, Outbound}
import gleamlang_ws_server/nats_transport.{Inbound}
import gleamlang_ws_server/pg_groups
import gleamlang_ws_server/registry
import gleeunit/should

@external(erlang, "gleamlang_ws_server_ffi", "self_node_binary")
fn self_node() -> String

@external(erlang, "erlang", "unique_integer")
fn unique_int_raw(opts: List(a)) -> Int

pub fn nats_inbound_from_self_does_not_fanout_test() -> Nil {
  let #(reg, fan) = setup()
  let ws = process.new_subject()
  registry.register(reg, ByConv("c-self"), ws)
  wait_for_registered(reg, ByConv("c-self"))

  nats_transport.dispatch_inbound_default(
    Inbound(
      subject: nats_transport.conv_subject("c-self"),
      payload: bit_array.from_string("hello"),
      headers: [#("Source-Node", self_node())],
      source_node: Some(self_node()),
    ),
    reg,
    fan,
  )

  expect_no_message(ws, 100)
}

pub fn nats_inbound_from_foreign_node_fans_out_test() -> Nil {
  let #(reg, fan) = setup()
  let ws = process.new_subject()
  registry.register(reg, ByConv("c-foreign"), ws)
  wait_for_registered(reg, ByConv("c-foreign"))

  nats_transport.dispatch_inbound_default(
    Inbound(
      subject: nats_transport.conv_subject("c-foreign"),
      payload: bit_array.from_string("hello"),
      headers: [#("Source-Node", "other@nowhere")],
      source_node: Some("other@nowhere"),
    ),
    reg,
    fan,
  )

  process.receive(ws, 500)
  |> should.equal(Ok(Outbound("hello")))
}

pub fn nats_inbound_without_source_node_fans_out_test() -> Nil {
  let #(reg, fan) = setup()
  let ws = process.new_subject()
  registry.register(reg, ByConv("c-anon"), ws)
  wait_for_registered(reg, ByConv("c-anon"))

  nats_transport.dispatch_inbound_default(
    Inbound(
      subject: nats_transport.conv_subject("c-anon"),
      payload: bit_array.from_string("anon"),
      headers: [],
      source_node: None,
    ),
    reg,
    fan,
  )

  process.receive(ws, 500)
  |> should.equal(Ok(Outbound("anon")))
}

fn setup() {
  pg_groups.ensure_started()
  let tag = int.to_string(unique_int_raw([]))
  let reg_name = fanout.stable_name("e2e_nats_reg_" <> tag)
  let assert Ok(reg_started) =
    registry.start(reg_name, "e2e_nats_reg_ets_" <> tag)
  let reg = reg_started.data
  let fan_name = fanout.stable_name("e2e_nats_fan_" <> tag)
  let assert Ok(fan_started) = fanout.start(fan_name, reg)
  #(reg, fan_started.data)
}

fn wait_for_registered(reg, group) {
  do_wait(reg, group, 50)
}

fn do_wait(reg, group, attempts: Int) -> Nil {
  case registry.members(reg, group) {
    [_, ..] -> Nil
    [] ->
      case attempts {
        0 -> should.fail()
        n -> {
          process.sleep(10)
          do_wait(reg, group, n - 1)
        }
      }
  }
}

fn expect_no_message(s, timeout_ms: Int) -> Nil {
  case process.receive(s, timeout_ms) {
    Ok(_) -> should.fail()
    Error(_) -> Nil
  }
}
