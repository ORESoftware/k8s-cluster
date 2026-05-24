//// Gleam port of `src/protocol.rs`. The wire format is camelCase.
////
//// Gleam custom types are real algebraic data types — every `case` over a
//// `Request` or `Response` value is checked for exhaustiveness by the
//// compiler. This is the strongest form of the "no magic strings" property
//// of any of the five clients we ship.

import gleam/dict.{type Dict}
import gleam/dynamic/decode
import gleam/json.{type Json}
import gleam/option.{type Option, None, Some}
import gleam/result

// ---------- Request ------------------------------------------------------

pub type Request {
  VersionRequest(uuid: String, value: String)
  AuthRequest(uuid: String, token: String)
  LockRequest(
    uuid: String,
    key: Option(String),
    keys: Option(List(String)),
    pid: Option(Int),
    ttl: Int,
    max: Option(Int),
    force: Bool,
    retry_count: Int,
    keep_locks_after_death: Bool,
  )
  UnlockRequest(
    uuid: String,
    key: Option(String),
    keys: Option(List(String)),
    lock_uuid: Option(String),
    force: Bool,
  )
  RegisterReadRequest(uuid: String, key: String)
  RegisterWriteRequest(uuid: String, key: String)
  EndReadRequest(uuid: String, key: String)
  EndWriteRequest(uuid: String, key: String)
  LockInfoRequest(uuid: String, key: String)
  LsRequest(uuid: String)
  HeartbeatRequest(uuid: String)
}

pub fn request_uuid(req: Request) -> String {
  case req {
    VersionRequest(uuid, ..) -> uuid
    AuthRequest(uuid, ..) -> uuid
    LockRequest(uuid, ..) -> uuid
    UnlockRequest(uuid, ..) -> uuid
    RegisterReadRequest(uuid, ..) -> uuid
    RegisterWriteRequest(uuid, ..) -> uuid
    EndReadRequest(uuid, ..) -> uuid
    EndWriteRequest(uuid, ..) -> uuid
    LockInfoRequest(uuid, ..) -> uuid
    LsRequest(uuid) -> uuid
    HeartbeatRequest(uuid) -> uuid
  }
}

fn opt_str(field: String, value: Option(String)) -> List(#(String, Json)) {
  case value {
    Some(v) -> [#(field, json.string(v))]
    None -> []
  }
}

fn opt_strs(field: String, value: Option(List(String))) -> List(#(String, Json)) {
  case value {
    Some(vs) -> [#(field, json.array(vs, json.string))]
    None -> []
  }
}

fn opt_int(field: String, value: Option(Int)) -> List(#(String, Json)) {
  case value {
    Some(v) -> [#(field, json.int(v))]
    None -> []
  }
}

pub fn encode_request(req: Request) -> String {
  let body = case req {
    VersionRequest(uuid, value) -> [
      #("type", json.string("version")),
      #("uuid", json.string(uuid)),
      #("value", json.string(value)),
    ]
    AuthRequest(uuid, token) -> [
      #("type", json.string("auth")),
      #("uuid", json.string(uuid)),
      #("token", json.string(token)),
    ]
    LockRequest(
      uuid,
      key,
      keys,
      pid,
      ttl,
      max,
      force,
      retry_count,
      keep_locks_after_death,
    ) -> {
      [
        #("type", json.string("lock")),
        #("uuid", json.string(uuid)),
        #("ttl", json.int(ttl)),
        #("force", json.bool(force)),
        #("retryCount", json.int(retry_count)),
        #("keepLocksAfterDeath", json.bool(keep_locks_after_death)),
      ]
      |> append(opt_str("key", key))
      |> append(opt_strs("keys", keys))
      |> append(opt_int("pid", pid))
      |> append(opt_int("max", max))
    }
    UnlockRequest(uuid, key, keys, lock_uuid, force) -> {
      [
        #("type", json.string("unlock")),
        #("uuid", json.string(uuid)),
        #("force", json.bool(force)),
      ]
      |> append(opt_str("key", key))
      |> append(opt_strs("keys", keys))
      |> append(opt_str("lockUuid", lock_uuid))
    }
    RegisterReadRequest(uuid, key) -> [
      #("type", json.string("registerRead")),
      #("uuid", json.string(uuid)),
      #("key", json.string(key)),
    ]
    RegisterWriteRequest(uuid, key) -> [
      #("type", json.string("registerWrite")),
      #("uuid", json.string(uuid)),
      #("key", json.string(key)),
    ]
    EndReadRequest(uuid, key) -> [
      #("type", json.string("endRead")),
      #("uuid", json.string(uuid)),
      #("key", json.string(key)),
    ]
    EndWriteRequest(uuid, key) -> [
      #("type", json.string("endWrite")),
      #("uuid", json.string(uuid)),
      #("key", json.string(key)),
    ]
    LockInfoRequest(uuid, key) -> [
      #("type", json.string("lockInfo")),
      #("uuid", json.string(uuid)),
      #("key", json.string(key)),
    ]
    LsRequest(uuid) -> [
      #("type", json.string("ls")),
      #("uuid", json.string(uuid)),
    ]
    HeartbeatRequest(uuid) -> [
      #("type", json.string("heartbeat")),
      #("uuid", json.string(uuid)),
    ]
  }
  json.to_string(json.object(body))
}

fn append(a: List(b), b: List(b)) -> List(b) {
  case b {
    [] -> a
    _ -> {
      let combined = list_concat(a, b)
      combined
    }
  }
}

@external(erlang, "lists", "append")
fn list_concat(a: List(b), b: List(b)) -> List(b)

// ---------- Response -----------------------------------------------------

pub type Response {
  VersionResponse(
    uuid: String,
    broker_version: String,
    ok: Bool,
    error: Option(String),
  )
  AuthResponse(uuid: String, ok: Bool, error: Option(String))
  LockResponse(
    uuid: String,
    key: String,
    acquired: Bool,
    lock_request_count: Int,
    lock_uuid: Option(String),
    fencing_token: Option(Int),
    readers_count: Option(Int),
    error: Option(String),
  )
  CompositeLockResponse(
    uuid: String,
    keys: List(String),
    acquired: Bool,
    lock_uuid: Option(String),
    fencing_tokens: Option(Dict(String, Int)),
    error: Option(String),
  )
  UnlockResponse(
    uuid: String,
    keys: List(String),
    unlocked: Bool,
    lock_request_count: Int,
    error: Option(String),
  )
  RegisterReadResultResponse(
    uuid: String,
    key: String,
    readers_count: Int,
    writer_flag: Bool,
    granted: Bool,
    lock_uuid: Option(String),
    fencing_token: Option(Int),
  )
  RegisterWriteResultResponse(
    uuid: String,
    key: String,
    readers_count: Int,
    writer_flag: Bool,
    granted: Bool,
    lock_uuid: Option(String),
    fencing_token: Option(Int),
  )
  EndReadResultResponse(uuid: String, key: String, readers_count: Int)
  EndWriteResultResponse(
    uuid: String,
    key: String,
    readers_count: Int,
    writer_flag: Bool,
  )
  LockInfoResponse(
    uuid: String,
    key: String,
    is_locked: Bool,
    lockholder_uuids: List(String),
    lock_request_count: Int,
    readers_count: Int,
    writer_flag: Bool,
  )
  LsResultResponse(uuid: String, keys: List(String))
  ReelectionResponse(uuid: String, key: String)
  ErrorResponse(uuid: String, error: String)
  OkResponse(uuid: String)
}

pub fn response_uuid(resp: Response) -> String {
  case resp {
    VersionResponse(uuid, ..) -> uuid
    AuthResponse(uuid, ..) -> uuid
    LockResponse(uuid, ..) -> uuid
    CompositeLockResponse(uuid, ..) -> uuid
    UnlockResponse(uuid, ..) -> uuid
    RegisterReadResultResponse(uuid, ..) -> uuid
    RegisterWriteResultResponse(uuid, ..) -> uuid
    EndReadResultResponse(uuid, ..) -> uuid
    EndWriteResultResponse(uuid, ..) -> uuid
    LockInfoResponse(uuid, ..) -> uuid
    LsResultResponse(uuid, ..) -> uuid
    ReelectionResponse(uuid, ..) -> uuid
    ErrorResponse(uuid, ..) -> uuid
    OkResponse(uuid) -> uuid
  }
}

fn dec_optional_string(
  name: String,
  next: fn(Option(String)) -> decode.Decoder(a),
) -> decode.Decoder(a) {
  decode.optional_field(name, None, decode.optional(decode.string), next)
}

fn dec_optional_int(
  name: String,
  next: fn(Option(Int)) -> decode.Decoder(a),
) -> decode.Decoder(a) {
  decode.optional_field(name, None, decode.optional(decode.int), next)
}

fn dec_string_default(
  name: String,
  default: String,
  next: fn(String) -> decode.Decoder(a),
) -> decode.Decoder(a) {
  decode.optional_field(name, default, decode.string, next)
}

fn dec_int_default(
  name: String,
  default: Int,
  next: fn(Int) -> decode.Decoder(a),
) -> decode.Decoder(a) {
  decode.optional_field(name, default, decode.int, next)
}

fn dec_bool_default(
  name: String,
  default: Bool,
  next: fn(Bool) -> decode.Decoder(a),
) -> decode.Decoder(a) {
  decode.optional_field(name, default, decode.bool, next)
}

fn dec_list_string(
  name: String,
  next: fn(List(String)) -> decode.Decoder(a),
) -> decode.Decoder(a) {
  decode.optional_field(name, [], decode.list(decode.string), next)
}

pub fn decode_response(line: String) -> Result(Response, String) {
  use decoded <- result.try(
    json.parse(line, response_decoder()) |> result.map_error(fn(_) { "json" }),
  )
  Ok(decoded)
}

fn response_decoder() -> decode.Decoder(Response) {
  use type_tag <- decode.field("type", decode.string)
  use uuid <- decode.field("uuid", decode.string)
  case type_tag {
    "version" -> {
      use bv <- dec_string_default("brokerVersion", "")
      use ok <- dec_bool_default("ok", False)
      use err <- dec_optional_string("error")
      decode.success(VersionResponse(uuid, bv, ok, err))
    }
    "auth" -> {
      use ok <- dec_bool_default("ok", False)
      use err <- dec_optional_string("error")
      decode.success(AuthResponse(uuid, ok, err))
    }
    "lock" -> {
      use key <- dec_string_default("key", "")
      use acquired <- dec_bool_default("acquired", False)
      use lrc <- dec_int_default("lockRequestCount", 0)
      use lu <- dec_optional_string("lockUuid")
      use ft <- dec_optional_int("fencingToken")
      use rc <- dec_optional_int("readersCount")
      use err <- dec_optional_string("error")
      decode.success(LockResponse(uuid, key, acquired, lrc, lu, ft, rc, err))
    }
    "compositeLock" -> {
      use keys <- dec_list_string("keys")
      use acquired <- dec_bool_default("acquired", False)
      use lu <- dec_optional_string("lockUuid")
      use ft <- decode.optional_field(
        "fencingTokens",
        None,
        decode.optional(decode.dict(decode.string, decode.int)),
      )
      use err <- dec_optional_string("error")
      decode.success(CompositeLockResponse(uuid, keys, acquired, lu, ft, err))
    }
    "unlock" -> {
      use keys <- dec_list_string("keys")
      use unlocked <- dec_bool_default("unlocked", False)
      use lrc <- dec_int_default("lockRequestCount", 0)
      use err <- dec_optional_string("error")
      decode.success(UnlockResponse(uuid, keys, unlocked, lrc, err))
    }
    "registerReadResult" -> {
      use key <- dec_string_default("key", "")
      use rc <- dec_int_default("readersCount", 0)
      use wf <- dec_bool_default("writerFlag", False)
      use granted <- dec_bool_default("granted", False)
      use lu <- dec_optional_string("lockUuid")
      use ft <- dec_optional_int("fencingToken")
      decode.success(
        RegisterReadResultResponse(uuid, key, rc, wf, granted, lu, ft),
      )
    }
    "registerWriteResult" -> {
      use key <- dec_string_default("key", "")
      use rc <- dec_int_default("readersCount", 0)
      use wf <- dec_bool_default("writerFlag", False)
      use granted <- dec_bool_default("granted", False)
      use lu <- dec_optional_string("lockUuid")
      use ft <- dec_optional_int("fencingToken")
      decode.success(
        RegisterWriteResultResponse(uuid, key, rc, wf, granted, lu, ft),
      )
    }
    "endReadResult" -> {
      use key <- dec_string_default("key", "")
      use rc <- dec_int_default("readersCount", 0)
      decode.success(EndReadResultResponse(uuid, key, rc))
    }
    "endWriteResult" -> {
      use key <- dec_string_default("key", "")
      use rc <- dec_int_default("readersCount", 0)
      use wf <- dec_bool_default("writerFlag", False)
      decode.success(EndWriteResultResponse(uuid, key, rc, wf))
    }
    "lockInfo" -> {
      use key <- dec_string_default("key", "")
      use is_locked <- dec_bool_default("isLocked", False)
      use lockholders <- dec_list_string("lockholderUuids")
      use lrc <- dec_int_default("lockRequestCount", 0)
      use rc <- dec_int_default("readersCount", 0)
      use wf <- dec_bool_default("writerFlag", False)
      decode.success(
        LockInfoResponse(uuid, key, is_locked, lockholders, lrc, rc, wf),
      )
    }
    "lsResult" -> {
      use keys <- dec_list_string("keys")
      decode.success(LsResultResponse(uuid, keys))
    }
    "reelection" -> {
      use key <- dec_string_default("key", "")
      decode.success(ReelectionResponse(uuid, key))
    }
    "error" -> {
      use err <- dec_string_default("error", "unknown")
      decode.success(ErrorResponse(uuid, err))
    }
    "ok" -> decode.success(OkResponse(uuid))
    _ -> decode.failure(OkResponse(uuid), "unknown response type")
  }
}
