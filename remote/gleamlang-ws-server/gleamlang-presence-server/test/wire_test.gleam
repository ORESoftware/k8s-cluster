//// Wire-frame encoder tests.
////
//// Asserts on the JSON shape produced by `wire.encode_*`. We parse the
//// produced string back to a dynamic value so the tests don't depend on
//// key ordering inside the JSON object — only on the (key, value) pairs
//// that are actually present.

import gleam/dict
import gleam/dynamic/decode
import gleam/json
import gleam/list
import gleam/option.{None, Some}
import gleam/string
import gleeunit/should
import gleamlang_presence_server/groups.{AddedToConv, RemovedFromConv}
import gleamlang_presence_server/wire

pub fn membership_changed_added_carries_members_test() -> Nil {
  let body =
    wire.encode_membership_changed("conv-1", AddedToConv(["alice", "bob", "carol"]))

  let pairs = parse_object(body)
  string_field(pairs, "type") |> should.equal("membership-changed")
  string_field(pairs, "change") |> should.equal("added")
  string_field(pairs, "conv") |> should.equal("conv-1")
  string_array_field(pairs, "members")
  |> should.equal(["alice", "bob", "carol"])
}

pub fn membership_changed_added_with_empty_members_is_valid_test() -> Nil {
  let body = wire.encode_membership_changed("c", AddedToConv([]))
  let pairs = parse_object(body)
  string_field(pairs, "change") |> should.equal("added")
  string_array_field(pairs, "members") |> should.equal([])
}

pub fn membership_changed_removed_has_no_members_field_test() -> Nil {
  let body = wire.encode_membership_changed("conv-9", RemovedFromConv)
  let pairs = parse_object(body)
  string_field(pairs, "type") |> should.equal("membership-changed")
  string_field(pairs, "change") |> should.equal("removed")
  string_field(pairs, "conv") |> should.equal("conv-9")
  dict.has_key(pairs, "members") |> should.equal(False)
}

pub fn kick_carries_reason_test() -> Nil {
  let body = wire.encode_kick("removed from conv conv-3")
  let pairs = parse_object(body)
  string_field(pairs, "type") |> should.equal("kick")
  string_field(pairs, "reason") |> should.equal("removed from conv conv-3")
}

pub fn re_registered_has_typed_envelope_test() -> Nil {
  let body = wire.encode_re_registered()
  let pairs = parse_object(body)
  string_field(pairs, "type") |> should.equal("re-registered")
}

pub fn encoders_round_trip_through_json_parse_test() -> Nil {
  // The encoded strings are valid JSON; `json.parse` should not error.
  let outputs = [
    wire.encode_membership_changed("c", AddedToConv(["u"])),
    wire.encode_membership_changed("c", RemovedFromConv),
    wire.encode_kick("any"),
    wire.encode_re_registered(),
    wire.encode_hello(
      user_id: "alice",
      conv_id: None,
      device_id: None,
      node: "presence@p0",
    ),
  ]
  list.each(outputs, fn(s) {
    json.parse(s, decode.dynamic) |> should.be_ok |> ignore
  })
}

pub fn hello_user_scope_has_null_conv_and_device_test() -> Nil {
  let body =
    wire.encode_hello(
      user_id: "alice",
      conv_id: None,
      device_id: None,
      node: "presence@p0",
    )
  let pairs = parse_object(body)
  string_field(pairs, "type") |> should.equal("hello")
  string_field(pairs, "scope") |> should.equal("user")
  string_field(pairs, "user") |> should.equal("alice")
  string_field(pairs, "node") |> should.equal("presence@p0")
  // `conv` and `device` are present and null (not missing) so the
  // client parser can stay branchless. Check the raw string rather
  // than relying on a particular dynamic-null decoder shape.
  contains(body, "\"conv\":null") |> should.be_true
  contains(body, "\"device\":null") |> should.be_true
}

pub fn hello_conv_scope_includes_conv_and_device_test() -> Nil {
  let body =
    wire.encode_hello(
      user_id: "alice",
      conv_id: Some("conv-1"),
      device_id: Some("dev-7"),
      node: "presence@p2",
    )
  let pairs = parse_object(body)
  string_field(pairs, "scope") |> should.equal("conv")
  string_field(pairs, "conv") |> should.equal("conv-1")
  string_field(pairs, "device") |> should.equal("dev-7")
  string_field(pairs, "node") |> should.equal("presence@p2")
}

// ── helpers ──────────────────────────────────────────────────────────────

type Pairs =
  dict.Dict(String, decode.Dynamic)

/// Parse the JSON string into a key→dynamic dict so test assertions can
/// look up individual fields independent of object key order.
fn parse_object(s: String) -> Pairs {
  let decoder = decode.dict(decode.string, decode.dynamic)
  json.parse(s, decoder)
  |> should.be_ok
}

fn string_field(pairs: Pairs, key: String) -> String {
  let assert Ok(value) = dict.get(pairs, key)
  decode.run(value, decode.string)
  |> should.be_ok
}

fn string_array_field(pairs: Pairs, key: String) -> List(String) {
  let assert Ok(value) = dict.get(pairs, key)
  decode.run(value, decode.list(decode.string))
  |> should.be_ok
}

fn contains(haystack: String, needle: String) -> Bool {
  string.contains(does: haystack, contain: needle)
}

fn ignore(_anything: a) -> Nil {
  Nil
}
