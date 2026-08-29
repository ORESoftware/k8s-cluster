//// Unit tests for NATS subject/header helpers and Erlang `dd_nats` FFI
//// gates. These do not open a NATS TCP socket.

import gleam/option.{None, Some}
import gleam_mcp_server/nats_wire
import gleeunit/should

pub fn safe_mcp_control_subject_is_accepted_test() -> Nil {
  nats_wire.subject_is_safe("dd.remote.mcp.control")
  |> should.be_true
}

pub fn crlf_in_subject_is_rejected_test() -> Nil {
  nats_wire.subject_is_safe("dd.remote.mcp.control\r\nPUB inject 0")
  |> should.be_false
}

pub fn space_in_subject_is_rejected_test() -> Nil {
  nats_wire.subject_is_safe("dd remote mcp")
  |> should.be_false
}

pub fn empty_subject_is_rejected_test() -> Nil {
  nats_wire.subject_is_safe("")
  |> should.be_false
}

pub fn header_get_is_case_insensitive_test() -> Nil {
  nats_wire.header_get([#("source-node", "a@host")], "Source-Node")
  |> should.equal(Some("a@host"))
}

pub fn source_node_self_is_already_delivered_test() -> Nil {
  nats_wire.source_node_already_delivered(
    Some("mcp@pod-0"),
    "mcp@pod-0",
    ["mcp@pod-1"],
  )
  |> should.be_true
}

pub fn source_node_foreign_is_kept_test() -> Nil {
  nats_wire.source_node_already_delivered(
    Some("other@cluster"),
    "mcp@pod-0",
    ["mcp@pod-1"],
  )
  |> should.be_false
}

pub fn source_node_absent_is_kept_test() -> Nil {
  nats_wire.source_node_already_delivered(None, "mcp@pod-0", ["mcp@pod-1"])
  |> should.be_false
}

@external(erlang, "dd_nats", "valid_subject")
fn dd_nats_valid_subject(subject: String) -> Bool

@external(erlang, "dd_nats", "json_escape")
fn dd_nats_json_escape(raw: String) -> String

@external(erlang, "dd_nats", "parse_url_host")
fn dd_nats_parse_url_host(url: String) -> Result(#(String, Int), String)

pub fn erlang_valid_subject_matches_gleam_test() -> Nil {
  dd_nats_valid_subject("dd.remote.mcp.control")
  |> should.be_true
  dd_nats_valid_subject("bad subject")
  |> should.be_false
}

pub fn erlang_json_escape_quotes_test() -> Nil {
  dd_nats_json_escape("a\"b\\c")
  |> should.equal("a\\\"b\\\\c")
}

pub fn erlang_parse_nats_url_test() -> Nil {
  dd_nats_parse_url_host("nats://127.0.0.1:4222")
  |> should.equal(Ok(#("127.0.0.1", 4222)))
}

pub fn erlang_parse_nats_url_rejects_tls_scheme_test() -> Nil {
  case dd_nats_parse_url_host("tls://127.0.0.1:4222") {
    Error(_) -> Nil
    Ok(_) -> should.fail()
  }
}
