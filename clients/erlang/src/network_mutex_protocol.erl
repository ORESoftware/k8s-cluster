-module(network_mutex_protocol).

-export([
    protocol_version/0,
    max_composite_keys/0,
    request_types/0,
    response_types/0,
    request_type_to_wire/1,
    response_type_from_wire/1,
    version_request/1,
    version_request/2,
    auth_request/2,
    lock_request_single/5,
    lock_request_composite/4,
    unlock_request_single/4,
    unlock_request_composite/3,
    rw_request/3,
    lock_info_request/2,
    ls_request/1,
    heartbeat_request/1
]).

protocol_version() -> "0.1.0".
max_composite_keys() -> 5.

request_types() ->
    [
        "version",
        "auth",
        "lock",
        "unlock",
        "registerRead",
        "registerWrite",
        "endRead",
        "endWrite",
        "lockInfo",
        "ls",
        "heartbeat"
    ].

response_types() ->
    [
        "version",
        "auth",
        "lock",
        "compositeLock",
        "unlock",
        "registerReadResult",
        "registerWriteResult",
        "endReadResult",
        "endWriteResult",
        "lockInfo",
        "lsResult",
        "reelection",
        "error",
        "ok"
    ].

request_type_to_wire(version) -> "version";
request_type_to_wire(auth) -> "auth";
request_type_to_wire(lock) -> "lock";
request_type_to_wire(unlock) -> "unlock";
request_type_to_wire(register_read) -> "registerRead";
request_type_to_wire(register_write) -> "registerWrite";
request_type_to_wire(end_read) -> "endRead";
request_type_to_wire(end_write) -> "endWrite";
request_type_to_wire(lock_info) -> "lockInfo";
request_type_to_wire(ls) -> "ls";
request_type_to_wire(heartbeat) -> "heartbeat".

response_type_from_wire("version") -> version;
response_type_from_wire("auth") -> auth;
response_type_from_wire("lock") -> lock;
response_type_from_wire("compositeLock") -> composite_lock;
response_type_from_wire("unlock") -> unlock;
response_type_from_wire("registerReadResult") -> register_read_result;
response_type_from_wire("registerWriteResult") -> register_write_result;
response_type_from_wire("endReadResult") -> end_read_result;
response_type_from_wire("endWriteResult") -> end_write_result;
response_type_from_wire("lockInfo") -> lock_info;
response_type_from_wire("lsResult") -> ls_result;
response_type_from_wire("reelection") -> reelection;
response_type_from_wire("error") -> error;
response_type_from_wire("ok") -> ok;
response_type_from_wire(_) -> unknown.

version_request(Uuid) -> version_request(Uuid, protocol_version()).

version_request(Uuid, Value) ->
    frame([{string, "type", request_type_to_wire(version)}, {string, "uuid", Uuid}, {string, "value", Value}]).

auth_request(Uuid, Token) ->
    frame([{string, "type", request_type_to_wire(auth)}, {string, "uuid", Uuid}, {string, "token", Token}]).

lock_request_single(Uuid, Key, TtlMs, MaxHolders, Wait) ->
    frame(
        compact([
            {string, "type", request_type_to_wire(lock)},
            {string, "uuid", Uuid},
            {string, "key", Key},
            optional_int("ttl", TtlMs, TtlMs > 0),
            optional_int("max", MaxHolders, is_integer(MaxHolders)),
            optional_bool("wait", Wait, is_boolean(Wait))
        ])
    ).

lock_request_composite(Uuid, Keys, TtlMs, Wait) ->
    Count = length(Keys),
    case Count >= 1 andalso Count =< ?MODULE:max_composite_keys() of
        true ->
            frame(
                compact([
                    {string, "type", request_type_to_wire(lock)},
                    {string, "uuid", Uuid},
                    {strings, "keys", Keys},
                    optional_int("ttl", TtlMs, TtlMs > 0),
                    optional_bool("wait", Wait, is_boolean(Wait))
                ])
            );
        false ->
            error({bad_composite_key_count, Count})
    end.

unlock_request_single(Uuid, Key, LockUuid, Force) ->
    frame(
        compact([
            {string, "type", request_type_to_wire(unlock)},
            {string, "uuid", Uuid},
            {string, "key", Key},
            optional_string("lockUuid", LockUuid, LockUuid =/= undefined andalso LockUuid =/= ""),
            optional_bool("force", Force, Force =:= true)
        ])
    ).

unlock_request_composite(Uuid, Keys, LockUuid) ->
    frame(
        compact([
            {string, "type", request_type_to_wire(unlock)},
            {string, "uuid", Uuid},
            {strings, "keys", Keys},
            optional_string("lockUuid", LockUuid, LockUuid =/= undefined andalso LockUuid =/= "")
        ])
    ).

rw_request(Type, Uuid, Key) ->
    frame([{string, "type", request_type_to_wire(Type)}, {string, "uuid", Uuid}, {string, "key", Key}]).

lock_info_request(Uuid, Key) -> rw_request(lock_info, Uuid, Key).

ls_request(Uuid) ->
    frame([{string, "type", request_type_to_wire(ls)}, {string, "uuid", Uuid}]).

heartbeat_request(Uuid) ->
    frame([{string, "type", request_type_to_wire(heartbeat)}, {string, "uuid", Uuid}]).

optional_string(Key, Value, true) -> {string, Key, Value};
optional_string(_, _, false) -> skip.

optional_int(Key, Value, true) -> {int, Key, Value};
optional_int(_, _, false) -> skip.

optional_bool(Key, Value, true) -> {bool, Key, Value};
optional_bool(_, _, false) -> skip.

compact(Fields) -> [Field || Field <- Fields, Field =/= skip].

frame(Fields) ->
    "{" ++ join([field_to_json(Field) || Field <- Fields], ",") ++ "}\n".

field_to_json({string, Key, Value}) ->
    quote(Key) ++ ":" ++ quote(Value);
field_to_json({int, Key, Value}) ->
    quote(Key) ++ ":" ++ integer_to_list(Value);
field_to_json({bool, Key, true}) ->
    quote(Key) ++ ":true";
field_to_json({bool, Key, false}) ->
    quote(Key) ++ ":false";
field_to_json({strings, Key, Values}) ->
    quote(Key) ++ ":[" ++ join([quote(Value) || Value <- Values], ",") ++ "]".

quote(Value) when is_binary(Value) -> quote(binary_to_list(Value));
quote(Value) ->
    "\"" ++ escape(Value) ++ "\"".

escape([]) -> [];
escape([$" | Rest]) -> [$\\, $" | escape(Rest)];
escape([$\\ | Rest]) -> [$\\, $\\ | escape(Rest)];
escape([$\n | Rest]) -> [$\\, $n | escape(Rest)];
escape([Char | Rest]) -> [Char | escape(Rest)].

join([], _) -> "";
join([One], _) -> One;
join([One | Rest], Sep) -> One ++ Sep ++ join(Rest, Sep).

