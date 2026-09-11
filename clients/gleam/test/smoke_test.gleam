//// Run with: `gleam test`. Executes against a broker on
//// LIVE_MUTEX_HOST/LIVE_MUTEX_PORT (defaults 127.0.0.1:6970). The smoke
//// test is gated on the env var so unit tests still pass without a broker.

import dd_rust_network_mutex_client as nm
import dd_rust_network_mutex_client/protocol as p
import gleam/dict
import gleam/int
import gleam/io
import gleam/option.{None, Some}
import gleam/string
import gleeunit
import gleeunit/should

pub fn main() {
  gleeunit.main()
}

pub fn encode_lock_request_uses_camel_case_test() {
  let req =
    p.LockRequest(
      uuid: "u",
      key: Some("k"),
      keys: None,
      pid: None,
      ttl: 1000,
      max: None,
      force: False,
      retry_count: 0,
      keep_locks_after_death: False,
      wait: Some(False),
    )
  let encoded = p.encode_request(req)
  should.equal(string.contains(encoded, "\"type\":\"lock\""), True)
  should.equal(string.contains(encoded, "\"keepLocksAfterDeath\":false"), True)
  should.equal(string.contains(encoded, "\"retryCount\":0"), True)
  should.equal(string.contains(encoded, "\"wait\":false"), True)
}

pub fn encode_lock_request_preserves_wait_true_and_omits_absent_wait_test() {
  let wait_true =
    p.LockRequest(
      uuid: "u-wait",
      key: Some("k"),
      keys: None,
      pid: None,
      ttl: 1000,
      max: None,
      force: False,
      retry_count: 0,
      keep_locks_after_death: False,
      wait: Some(True),
    )
    |> p.encode_request
  let wait_omitted =
    p.LockRequest(
      uuid: "u-omit",
      key: Some("k"),
      keys: None,
      pid: None,
      ttl: 1000,
      max: None,
      force: False,
      retry_count: 0,
      keep_locks_after_death: False,
      wait: None,
    )
    |> p.encode_request

  should.equal(string.contains(wait_true, "\"wait\":true"), True)
  should.equal(string.contains(wait_omitted, "\"wait\""), False)
}

pub fn decode_composite_lock_response_test() {
  let raw =
    "{\"type\":\"compositeLock\",\"uuid\":\"u\",\"keys\":[\"a\",\"b\"],\"acquired\":true,\"lockUuid\":\"L\",\"fencingTokens\":{\"a\":1,\"b\":2}}"
  let assert Ok(p.CompositeLockResponse(_, _, True, Some(lu), Some(_), _)) =
    p.decode_response(raw)
  should.equal(lu, "L")
}

pub fn smoke_lifecycle_test() {
  case env_or_empty("LIVE_MUTEX_SMOKE") {
    "1" -> run_smoke()
    _ -> {
      io.println("[smoke-gleam] skipped (set LIVE_MUTEX_SMOKE=1)")
      Nil
    }
  }
}

fn run_smoke() -> Nil {
  let host = env_or("LIVE_MUTEX_HOST", "127.0.0.1")
  let port = case int.parse(env_or("LIVE_MUTEX_PORT", "6970")) {
    Ok(n) -> n
    Error(_) -> 6970
  }
  let assert Ok(client) = nm.connect(host, port, None)
  io.println("[smoke-gleam] connected " <> host)

  let assert Ok(handle) = nm.acquire(client, "smoke-gleam-exclusive", 5000)
  io.println("[smoke-gleam] exclusive grant: " <> handle.lock_uuid)
  let assert Ok(_) = nm.release_single(client, handle)
  io.println("[smoke-gleam] released exclusive")

  let assert Ok(comp) =
    nm.acquire_many(client, ["smoke-gleam-a", "smoke-gleam-b"], 5000)
  let token_count = dict.size(comp.fencing_tokens)
  io.println(
    "[smoke-gleam] composite grant: "
    <> comp.lock_uuid
    <> " ("
    <> int.to_string(token_count)
    <> " tokens)",
  )
  let assert Ok(_) = nm.release_composite(client, comp)
  io.println("[smoke-gleam] released composite")

  let assert Ok(#(_, _)) = nm.acquire_write(client, "smoke-gleam-rw")
  let assert Ok(_) = nm.release_write(client, "smoke-gleam-rw")

  nm.close(client)
  io.println("[smoke-gleam] OK")
}

@external(erlang, "dd_rust_network_mutex_client_ffi_helpers", "getenv")
fn ffi_getenv(name: String) -> Result(String, Nil)

fn env_or(name: String, default: String) -> String {
  case ffi_getenv(name) {
    Ok(v) -> v
    Error(_) -> default
  }
}

fn env_or_empty(name: String) -> String {
  env_or(name, "")
}
