//// NATS client URL parsing (no broker required). Live broker tests
//// belong in CI with NATS_URL; this file always runs.

import gleam_mcp_server/nats_wire
import gleeunit/should

@external(erlang, "dd_nats", "parse_url_host")
fn dd_nats_parse_url_host(url: String) -> Result(#(String, Int), String)

pub fn default_dev_nats_url_parses_test() -> Nil {
  nats_wire.subject_is_safe("dd.remote.mcp.control")
  |> should.be_true
  dd_nats_parse_url_host("nats://127.0.0.1:4222")
  |> should.equal(Ok(#("127.0.0.1", 4222)))
}
