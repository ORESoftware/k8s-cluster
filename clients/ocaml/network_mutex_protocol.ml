let protocol_version = "0.1.0"
let max_composite_keys = 5

type request_type =
  | Version
  | Auth
  | Lock
  | Unlock
  | RegisterRead
  | RegisterWrite
  | EndRead
  | EndWrite
  | LockInfo
  | Ls
  | Heartbeat

type response_type =
  | VersionResponse
  | AuthResponse
  | LockResponse
  | CompositeLockResponse
  | UnlockResponse
  | RegisterReadResult
  | RegisterWriteResult
  | EndReadResult
  | EndWriteResult
  | LockInfoResponse
  | LsResult
  | Reelection
  | ErrorResponse
  | OkResponse
  | UnknownResponse

let request_types =
  [
    "version";
    "auth";
    "lock";
    "unlock";
    "registerRead";
    "registerWrite";
    "endRead";
    "endWrite";
    "lockInfo";
    "ls";
    "heartbeat";
  ]

let response_types =
  [
    "version";
    "auth";
    "lock";
    "compositeLock";
    "unlock";
    "registerReadResult";
    "registerWriteResult";
    "endReadResult";
    "endWriteResult";
    "lockInfo";
    "lsResult";
    "reelection";
    "error";
    "ok";
  ]

let request_type_to_wire = function
  | Version -> "version"
  | Auth -> "auth"
  | Lock -> "lock"
  | Unlock -> "unlock"
  | RegisterRead -> "registerRead"
  | RegisterWrite -> "registerWrite"
  | EndRead -> "endRead"
  | EndWrite -> "endWrite"
  | LockInfo -> "lockInfo"
  | Ls -> "ls"
  | Heartbeat -> "heartbeat"

let response_type_from_wire = function
  | "version" -> VersionResponse
  | "auth" -> AuthResponse
  | "lock" -> LockResponse
  | "compositeLock" -> CompositeLockResponse
  | "unlock" -> UnlockResponse
  | "registerReadResult" -> RegisterReadResult
  | "registerWriteResult" -> RegisterWriteResult
  | "endReadResult" -> EndReadResult
  | "endWriteResult" -> EndWriteResult
  | "lockInfo" -> LockInfoResponse
  | "lsResult" -> LsResult
  | "reelection" -> Reelection
  | "error" -> ErrorResponse
  | "ok" -> OkResponse
  | _ -> UnknownResponse

let json_escape s =
  let b = Buffer.create (String.length s) in
  String.iter
    (function
      | '"' -> Buffer.add_string b "\\\""
      | '\\' -> Buffer.add_string b "\\\\"
      | '\n' -> Buffer.add_string b "\\n"
      | c -> Buffer.add_char b c)
    s;
  Buffer.contents b

let quote s = "\"" ^ json_escape s ^ "\""
let field_string key value = quote key ^ ":" ^ quote value
let field_int key value = quote key ^ ":" ^ string_of_int value
let field_bool key value = quote key ^ ":" ^ if value then "true" else "false"
let field_strings key values = quote key ^ ":[" ^ String.concat "," (List.map quote values) ^ "]"
let frame fields = "{" ^ String.concat "," fields ^ "}\n"

let version_request ?(value = protocol_version) uuid =
  frame [ field_string "type" "version"; field_string "uuid" uuid; field_string "value" value ]

let auth_request uuid token =
  frame [ field_string "type" "auth"; field_string "uuid" uuid; field_string "token" token ]

let lock_request_single ?pid ?(ttl = 0) ?max ?wait uuid key =
  let fields = [ field_string "type" "lock"; field_string "uuid" uuid; field_string "key" key ] in
  let fields = match pid with Some value -> fields @ [ field_int "pid" value ] | None -> fields in
  let fields = if ttl > 0 then fields @ [ field_int "ttl" ttl ] else fields in
  let fields = match max with Some value -> fields @ [ field_int "max" value ] | None -> fields in
  let fields = match wait with Some value -> fields @ [ field_bool "wait" value ] | None -> fields in
  frame fields

let lock_request_composite ?(ttl = 0) ?wait uuid keys =
  let count = List.length keys in
  if count < 1 || count > max_composite_keys then invalid_arg "composite key count must be 1..=5";
  let fields = [ field_string "type" "lock"; field_string "uuid" uuid; field_strings "keys" keys ] in
  let fields = if ttl > 0 then fields @ [ field_int "ttl" ttl ] else fields in
  let fields = match wait with Some value -> fields @ [ field_bool "wait" value ] | None -> fields in
  frame fields

let unlock_request_single ?(force = false) uuid key lock_uuid =
  let fields = [ field_string "type" "unlock"; field_string "uuid" uuid; field_string "key" key ] in
  let fields = if lock_uuid <> "" then fields @ [ field_string "lockUuid" lock_uuid ] else fields in
  let fields = if force then fields @ [ field_bool "force" true ] else fields in
  frame fields

let unlock_request_composite uuid keys lock_uuid =
  let fields = [ field_string "type" "unlock"; field_string "uuid" uuid; field_strings "keys" keys ] in
  let fields = if lock_uuid <> "" then fields @ [ field_string "lockUuid" lock_uuid ] else fields in
  frame fields

let rw_request request_type uuid key =
  frame [ field_string "type" (request_type_to_wire request_type); field_string "uuid" uuid; field_string "key" key ]

let lock_info_request uuid key = rw_request LockInfo uuid key
let ls_request uuid = frame [ field_string "type" "ls"; field_string "uuid" uuid ]
let heartbeat_request uuid = frame [ field_string "type" "heartbeat"; field_string "uuid" uuid ]

