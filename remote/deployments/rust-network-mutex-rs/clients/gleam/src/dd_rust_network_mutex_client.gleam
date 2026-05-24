//// Top-level Gleam client. The transport is the Erlang FFI in
//// `dd_rust_network_mutex_client_ffi.erl` (uses `gen_tcp` in `{packet,
//// line}` mode so a single recv returns one full JSON frame).
////
//// The client is *synchronous* per call (one outstanding request at a time
//// per connection) — that's enough for cross-runtime smoke testing and
//// keeps the Gleam side small. For high-fanout workloads you'd open
//// multiple connections in parallel from a supervisor.

import dd_rust_network_mutex_client/protocol.{
  type Request, type Response, AuthRequest, CompositeLockResponse,
  EndReadRequest, EndWriteRequest, LockRequest, LockResponse,
  RegisterReadRequest, RegisterReadResultResponse, RegisterWriteRequest,
  RegisterWriteResultResponse, ReelectionResponse, UnlockRequest,
  UnlockResponse,
}
import gleam/dict.{type Dict}
import gleam/list
import gleam/option.{type Option, None, Some}
import gleam/result

pub type Connection

@external(erlang, "dd_rust_network_mutex_client_ffi", "connect")
fn ffi_connect(
  host: String,
  port: Int,
  timeout_ms: Int,
) -> Result(Connection, String)

@external(erlang, "dd_rust_network_mutex_client_ffi", "close")
fn ffi_close(conn: Connection) -> Nil

@external(erlang, "dd_rust_network_mutex_client_ffi", "send_line")
fn ffi_send_line(conn: Connection, line: String) -> Result(Nil, String)

@external(erlang, "dd_rust_network_mutex_client_ffi", "recv_line")
fn ffi_recv_line(conn: Connection, timeout_ms: Int) -> Result(String, String)

@external(erlang, "dd_rust_network_mutex_client_ffi", "new_uuid")
pub fn new_uuid() -> String

pub type Client {
  Client(conn: Connection)
}

pub type SingleLockHandle {
  SingleLockHandle(key: String, lock_uuid: String, fencing_token: Int)
}

pub type CompositeLockHandle {
  CompositeLockHandle(
    keys: List(String),
    lock_uuid: String,
    fencing_tokens: Dict(String, Int),
  )
}

pub fn connect(
  host: String,
  port: Int,
  token: Option(String),
) -> Result(Client, String) {
  use conn <- result.try(ffi_connect(host, port, 5000))
  let client = Client(conn)
  case token {
    None -> Ok(client)
    Some(t) -> {
      let req = AuthRequest(uuid: new_uuid(), token: t)
      use _ <- result.try(send_and_recv(client, req, 5000))
      Ok(client)
    }
  }
}

pub fn close(client: Client) -> Nil {
  ffi_close(client.conn)
}

fn send_and_recv(
  client: Client,
  req: Request,
  timeout_ms: Int,
) -> Result(Response, String) {
  let line = protocol.encode_request(req)
  use _ <- result.try(ffi_send_line(client.conn, line))
  use raw <- result.try(ffi_recv_line(client.conn, timeout_ms))
  protocol.decode_response(raw)
}

fn send_until_grant(
  client: Client,
  req: Request,
  timeout_ms: Int,
) -> Result(Response, String) {
  let line = protocol.encode_request(req)
  use _ <- result.try(ffi_send_line(client.conn, line))
  recv_until_grant(client, timeout_ms)
}

fn recv_until_grant(
  client: Client,
  timeout_ms: Int,
) -> Result(Response, String) {
  use raw <- result.try(ffi_recv_line(client.conn, timeout_ms))
  use resp <- result.try(protocol.decode_response(raw))
  case resp {
    LockResponse(_, _, True, _, _, _, _, _) -> Ok(resp)
    LockResponse(_, _, _, _, _, _, _, Some(_)) -> Ok(resp)
    LockResponse(_, _, _, _, _, _, _, _) -> recv_until_grant(client, timeout_ms)
    CompositeLockResponse(_, _, True, _, _, _) -> Ok(resp)
    CompositeLockResponse(_, _, _, _, _, Some(_)) -> Ok(resp)
    CompositeLockResponse(_, _, _, _, _, _) ->
      recv_until_grant(client, timeout_ms)
    RegisterReadResultResponse(_, _, _, _, True, _, _) -> Ok(resp)
    RegisterReadResultResponse(_, _, _, _, _, _, _) ->
      recv_until_grant(client, timeout_ms)
    RegisterWriteResultResponse(_, _, _, _, True, _, _) -> Ok(resp)
    RegisterWriteResultResponse(_, _, _, _, _, _, _) ->
      recv_until_grant(client, timeout_ms)
    ReelectionResponse(_, _) -> recv_until_grant(client, timeout_ms)
    other -> Ok(other)
  }
}

pub fn acquire(
  client: Client,
  key: String,
  ttl_ms: Int,
) -> Result(SingleLockHandle, String) {
  let req =
    LockRequest(
      uuid: new_uuid(),
      key: Some(key),
      keys: None,
      pid: None,
      ttl: ttl_ms,
      max: None,
      force: False,
      retry_count: 0,
      keep_locks_after_death: False,
    )
  use resp <- result.try(send_until_grant(client, req, 30_000))
  case resp {
    LockResponse(_, k, True, _, Some(lu), Some(ft), _, _) ->
      Ok(SingleLockHandle(k, lu, ft))
    LockResponse(_, _, True, _, Some(lu), None, _, _) ->
      Ok(SingleLockHandle(key, lu, 0))
    other -> Error(format_unexpected("acquire", other))
  }
}

pub fn acquire_many(
  client: Client,
  keys: List(String),
  ttl_ms: Int,
) -> Result(CompositeLockHandle, String) {
  case list.length(keys) {
    n if n < 1 || n > 5 -> Error("composite key count must be 1..=5")
    _ -> {
      let req =
        LockRequest(
          uuid: new_uuid(),
          key: None,
          keys: Some(keys),
          pid: None,
          ttl: ttl_ms,
          max: None,
          force: False,
          retry_count: 0,
          keep_locks_after_death: False,
        )
      use resp <- result.try(send_until_grant(client, req, 30_000))
      case resp {
        CompositeLockResponse(_, ks, True, Some(lu), Some(ft), _) ->
          Ok(CompositeLockHandle(ks, lu, ft))
        CompositeLockResponse(_, ks, True, Some(lu), None, _) ->
          Ok(CompositeLockHandle(ks, lu, dict.new()))
        other -> Error(format_unexpected("acquire_many", other))
      }
    }
  }
}

pub fn release_single(
  client: Client,
  handle: SingleLockHandle,
) -> Result(Nil, String) {
  let req =
    UnlockRequest(
      uuid: new_uuid(),
      key: Some(handle.key),
      keys: None,
      lock_uuid: Some(handle.lock_uuid),
      force: False,
    )
  use resp <- result.try(send_and_recv(client, req, 5000))
  case resp {
    UnlockResponse(_, _, True, _, _) -> Ok(Nil)
    other -> Error(format_unexpected("release", other))
  }
}

pub fn release_composite(
  client: Client,
  handle: CompositeLockHandle,
) -> Result(Nil, String) {
  let req =
    UnlockRequest(
      uuid: new_uuid(),
      key: None,
      keys: Some(handle.keys),
      lock_uuid: Some(handle.lock_uuid),
      force: False,
    )
  use resp <- result.try(send_and_recv(client, req, 5000))
  case resp {
    UnlockResponse(_, _, True, _, _) -> Ok(Nil)
    other -> Error(format_unexpected("release", other))
  }
}

pub fn acquire_read(client: Client, key: String) -> Result(#(String, Int), String) {
  let req = RegisterReadRequest(uuid: new_uuid(), key: key)
  use resp <- result.try(send_until_grant(client, req, 30_000))
  case resp {
    RegisterReadResultResponse(_, _, _, _, True, lu, ft) ->
      Ok(#(option.unwrap(lu, ""), option.unwrap(ft, 0)))
    other -> Error(format_unexpected("acquire_read", other))
  }
}

pub fn release_read(client: Client, key: String) -> Result(Nil, String) {
  let req = EndReadRequest(uuid: new_uuid(), key: key)
  use _ <- result.try(send_and_recv(client, req, 5000))
  Ok(Nil)
}

pub fn acquire_write(
  client: Client,
  key: String,
) -> Result(#(String, Int), String) {
  let req = RegisterWriteRequest(uuid: new_uuid(), key: key)
  use resp <- result.try(send_until_grant(client, req, 30_000))
  case resp {
    RegisterWriteResultResponse(_, _, _, _, True, lu, ft) ->
      Ok(#(option.unwrap(lu, ""), option.unwrap(ft, 0)))
    other -> Error(format_unexpected("acquire_write", other))
  }
}

pub fn release_write(client: Client, key: String) -> Result(Nil, String) {
  let req = EndWriteRequest(uuid: new_uuid(), key: key)
  use _ <- result.try(send_and_recv(client, req, 5000))
  Ok(Nil)
}

fn format_unexpected(op: String, _resp: Response) -> String {
  op <> ": unexpected response variant"
}
