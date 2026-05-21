%%% FFI for the Gleam runtime-config receiver helper.
%%%
%%% Storage: `persistent_term:put({?MODULE, snapshot}, Map)` so the snapshot
%%% read on every `GET /internal/runtime-config` is a copy of a process-local
%%% reference (no cross-process round-trip, no ETS contention).
%%%
%%% Registration: `start_registration/0` spawns a single process per node that
%%% POSTs the subscriber record to the control plane and retries with
%%% exponential backoff (15s -> 5min cap). Re-calling is idempotent: a global
%%% registered name guards against duplicate spawn.
%%%
%%% HTTP: raw `gen_tcp` so we don't pull in inets / ssl just for one outbound
%%% call. Matches the pattern in `gleamlang_server_env.erl`.

-module(dd_runtime_config_client_ffi).

-export([
    start_registration/0,
    snapshot_json/0,
    apply_payload/1,
    reset/0,
    auth_ok/1,
    server_secret/0
]).

-define(SNAPSHOT_KEY, {?MODULE, snapshot}).
-define(REG_NAME, dd_runtime_config_client_register).
-define(REGISTER_BACKOFF_MS, 15_000).
-define(REGISTER_MAX_BACKOFF_MS, 300_000).
-define(REGISTER_TIMEOUT_MS, 10_000).

%% ---------- public API ---------------------------------------------------

start_registration() ->
    case whereis(?REG_NAME) of
        undefined ->
            spawn(fun() ->
                register(?REG_NAME, self()),
                register_loop(?REGISTER_BACKOFF_MS)
            end),
            nil;
        _Pid ->
            nil
    end.

snapshot_json() ->
    Snapshot = current_snapshot(),
    encode_snapshot(Snapshot).

%% Accepts a binary JSON body. Returns `{ok, ResponseJson}` on success or
%% `{error, ReasonBin}` on parse failure. The JSON shape is fixed so we get
%% away with a tiny hand-rolled parser instead of dragging in a JSON library.
apply_payload(Body) when is_binary(Body) ->
    case has_json_key(Body, <<"\"snapshot\"">>) andalso has_json_key(Body, <<"\"entries\"">>) of
        false ->
            {error, <<"snapshot is required">>};
        true ->
            PushId = string_field(Body, <<"\"pushId\"">>),
            Reason = string_field(Body, <<"\"reason\"">>),
            Version = int_field(Body, <<"\"snapshotVersion\"">>),
            Entries = extract_entries(Body),
            AppliedAt = iso_now(),
            Prev = case current_snapshot() of
                #{snapshot_version := V} -> V;
                _ -> 0
            end,
            NewVersion = case Version of
                undefined -> 0;
                Int -> Int
            end,
            case NewVersion < Prev of
                true ->
                    Response = io_lib:format(
                        "{\"ok\":true,\"service\":\"~ts\",\"appliedAt\":\"~ts\",\"appliedVersion\":~p,\"previousVersion\":~p,\"stale\":true,\"ignoredVersion\":~p}",
                        [service_name(), AppliedAt, Prev, Prev, NewVersion]
                    ),
                    {ok, iolist_to_binary(Response)};
                false ->
                    NewSnapshot = #{
                        snapshot_version => NewVersion,
                        entries => Entries,
                        applied_at => AppliedAt,
                        last_push_id => PushId,
                        last_reason => Reason,
                        service => service_name(),
                        scope => scope_name(),
                        env => env_label()
                    },
                    persistent_term:put(?SNAPSHOT_KEY, NewSnapshot),
                    Response = io_lib:format(
                        "{\"ok\":true,\"service\":\"~ts\",\"appliedAt\":\"~ts\",\"appliedVersion\":~p,\"previousVersion\":~p}",
                        [service_name(), AppliedAt, NewVersion, Prev]
                    ),
                    {ok, iolist_to_binary(Response)}
            end
    end.

reset() ->
    persistent_term:erase(?SNAPSHOT_KEY),
    nil.

auth_ok(Provided) when is_binary(Provided) ->
    case server_secret() of
        undefined -> allow_unauthenticated();
        Secret -> constant_time_equal(Provided, Secret)
    end;
auth_ok(Provided) when is_list(Provided) ->
    auth_ok(list_to_binary(Provided));
auth_ok(_) ->
    auth_ok(<<>>).

server_secret() ->
    case os:getenv("RUNTIME_CONFIG_SERVER_SECRET") of
        false -> undefined;
        "" -> undefined;
        Val -> list_to_binary(Val)
    end.

allow_unauthenticated() ->
    case os:getenv("RUNTIME_CONFIG_ALLOW_UNAUTHENTICATED") of
        "1" -> true;
        "true" -> true;
        "TRUE" -> true;
        "yes" -> true;
        "YES" -> true;
        _ -> false
    end.

%% ---------- registration loop -------------------------------------------

register_loop(Delay) ->
    case env(<<"RUNTIME_CONFIG_REGISTER_URL">>) of
        undefined ->
            io:format("[runtime-config] RUNTIME_CONFIG_REGISTER_URL not set; skipping~n");
        RegisterUrl ->
            case env(<<"RUNTIME_CONFIG_APPLY_URL">>) of
                undefined ->
                    io:format("[runtime-config] RUNTIME_CONFIG_APPLY_URL not set; skipping~n");
                ApplyUrl ->
                    Body = registration_payload(ApplyUrl),
                    case post_register(binary_to_list(RegisterUrl), Body) of
                        ok ->
                            io:format(
                                "[runtime-config] registered with control plane at ~ts~n",
                                [RegisterUrl]
                            );
                        {error, Reason} ->
                            io:format(
                                "[runtime-config] register error: ~p; retrying in ~p s~n",
                                [Reason, Delay div 1000]
                            ),
                            timer:sleep(Delay),
                            register_loop(min(Delay * 2, ?REGISTER_MAX_BACKOFF_MS))
                    end
            end
    end.

registration_payload(ApplyUrl) ->
    Body = io_lib:format(
        "{\"env\":\"~ts\",\"name\":\"~ts\",\"scope\":\"~ts\",\"applyUrl\":\"~ts\"}",
        [env_label(), service_name(), scope_name(), ApplyUrl]
    ),
    iolist_to_binary(Body).

post_register(Url, Body) ->
    case parse_http_url(Url) of
        {ok, Host, Port, Path} ->
            case gen_tcp:connect(Host, Port, [binary, {active, false}], 5000) of
                {ok, Socket} ->
                    Secret = case server_secret() of
                        undefined -> "";
                        S -> ["x-server-auth: ", S, "\r\n"]
                    end,
                    Req = [
                        "POST ", Path, " HTTP/1.1\r\n",
                        "Host: ", Host, ":", integer_to_list(Port), "\r\n",
                        "Connection: close\r\n",
                        "Content-Type: application/json\r\n",
                        Secret,
                        "Content-Length: ", integer_to_list(byte_size(Body)), "\r\n",
                        "\r\n",
                        Body
                    ],
                    case gen_tcp:send(Socket, Req) of
                        ok ->
                            Result = recv_status(Socket),
                            gen_tcp:close(Socket),
                            case Result of
                                {ok, Status} when Status >= 200, Status < 300 -> ok;
                                {ok, Status} -> {error, {http_status, Status}};
                                Other -> Other
                            end;
                        Send -> Send
                    end;
                {error, _} = ConnErr ->
                    ConnErr
            end;
        Err -> Err
    end.

parse_http_url(Url) ->
    try uri_string:parse(Url) of
        #{scheme := "http", host := Host} = Parsed ->
            Path0 = maps:get(path, Parsed, "/"),
            Path = case maps:get(query, Parsed, undefined) of
                undefined -> Path0;
                Query -> Path0 ++ "?" ++ Query
            end,
            {ok, Host, maps:get(port, Parsed, 80), Path};
        _ ->
            {error, unsupported_url}
    catch _:_ -> {error, parse_failed}
    end.

recv_status(Socket) ->
    case gen_tcp:recv(Socket, 0, ?REGISTER_TIMEOUT_MS) of
        {ok, Resp} ->
            case binary:split(Resp, <<"\r\n">>) of
                [StatusLine | _] ->
                    case binary:split(StatusLine, <<" ">>, [global]) of
                        [_Http, StatusBin | _] ->
                            try {ok, binary_to_integer(StatusBin)}
                            catch _:_ -> {error, bad_status}
                            end;
                        _ -> {error, bad_status}
                    end;
                _ -> {error, no_status}
            end;
        {error, _} = Err -> Err
    end.

%% ---------- snapshot helpers --------------------------------------------

current_snapshot() ->
    case persistent_term:get(?SNAPSHOT_KEY, undefined) of
        undefined ->
            #{
                snapshot_version => 0,
                entries => [],
                applied_at => null,
                last_push_id => null,
                last_reason => null,
                service => service_name(),
                scope => scope_name(),
                env => env_label()
            };
        Snap -> Snap
    end.

encode_snapshot(#{
    snapshot_version := V,
    entries := Entries,
    applied_at := AppliedAt,
    last_push_id := PushId,
    last_reason := Reason,
    service := Service,
    scope := Scope,
    env := Env
}) ->
    EntryBin = entries_to_json(Entries),
    AppliedAtJson = nullable_string(AppliedAt),
    PushIdJson = nullable_string(PushId),
    ReasonJson = nullable_string(Reason),
    iolist_to_binary(io_lib:format(
        "{\"service\":\"~ts\",\"scope\":\"~ts\",\"env\":\"~ts\",\"snapshotVersion\":~p,\"appliedAt\":~ts,\"lastPushId\":~ts,\"lastReason\":~ts,\"entries\":~ts}",
        [Service, Scope, Env, V, AppliedAtJson, PushIdJson, ReasonJson, EntryBin]
    )).

%% `entries` is stored as `[{Key :: binary(), ValueJson :: binary()}]`
%% where ValueJson is the raw JSON-encoded value from the push payload.
%% That way we round-trip whatever type the control plane sent (string,
%% number, bool, array, object) without needing a full JSON parser here.
entries_to_json(Entries) ->
    Pairs = [
        ["\"", escape_string(K), "\":", V]
        || {K, V} <- Entries
    ],
    Joined = lists:join(",", Pairs),
    [${, Joined, $}].

nullable_string(null) -> <<"null">>;
nullable_string(undefined) -> <<"null">>;
nullable_string(Value) when is_binary(Value) -> [<<"\"">>, escape_string(Value), <<"\"">>];
nullable_string(Value) when is_list(Value) -> nullable_string(iolist_to_binary(Value)).

escape_string(Value) when is_binary(Value) ->
    binary:replace(Value, <<"\"">>, <<"\\\"">>, [global]).

%% ---------- payload parsing (small, dependency-free) -------------------

string_field(Body, Key) ->
    case binary:match(Body, Key) of
        nomatch -> undefined;
        {Start, KeyLen} ->
            Tail = binary:part(Body, Start + KeyLen, byte_size(Body) - Start - KeyLen),
            case find_quoted_value(Tail) of
                undefined -> undefined;
                {ok, Bin} -> Bin
            end
    end.

int_field(Body, Key) ->
    case binary:match(Body, Key) of
        nomatch -> undefined;
        {Start, KeyLen} ->
            Tail = binary:part(Body, Start + KeyLen, byte_size(Body) - Start - KeyLen),
            case find_int_value(Tail) of
                undefined -> undefined;
                Int -> Int
            end
    end.

has_json_key(Body, Key) ->
    binary:match(Body, Key) =/= nomatch.

find_quoted_value(<<>>) -> undefined;
find_quoted_value(<<$\s, Rest/binary>>) -> find_quoted_value(Rest);
find_quoted_value(<<$\t, Rest/binary>>) -> find_quoted_value(Rest);
find_quoted_value(<<$\r, Rest/binary>>) -> find_quoted_value(Rest);
find_quoted_value(<<$\n, Rest/binary>>) -> find_quoted_value(Rest);
find_quoted_value(<<$:, Rest/binary>>) -> find_quoted_value(Rest);
find_quoted_value(<<$", Rest/binary>>) ->
    {ok, take_until_quote(Rest, <<>>)};
find_quoted_value(_) -> undefined.

take_until_quote(<<>>, Acc) -> Acc;
take_until_quote(<<$\\, $", Rest/binary>>, Acc) ->
    take_until_quote(Rest, <<Acc/binary, $">>);
take_until_quote(<<$", _Rest/binary>>, Acc) -> Acc;
take_until_quote(<<C, Rest/binary>>, Acc) ->
    take_until_quote(Rest, <<Acc/binary, C>>).

find_int_value(<<>>) -> undefined;
find_int_value(<<$\s, Rest/binary>>) -> find_int_value(Rest);
find_int_value(<<$:, Rest/binary>>) -> find_int_value(Rest);
find_int_value(Bin) ->
    {Digits, _} = take_digits(Bin, <<>>),
    case byte_size(Digits) of
        0 -> undefined;
        _ -> binary_to_integer(Digits)
    end.

take_digits(<<C, Rest/binary>>, Acc) when C >= $0, C =< $9 ->
    take_digits(Rest, <<Acc/binary, C>>);
take_digits(<<$-, Rest/binary>>, <<>>) ->
    take_digits(Rest, <<$->>);
take_digits(Rest, Acc) -> {Acc, Rest}.

%% Returns `[{KeyBin, ValueJsonBin}]` extracted from the `entries:[...]`
%% array. We do a single pass and slice out each `{ ... }` object's `key`
%% literal and re-serialise the matching `value` field as-is.
extract_entries(Body) ->
    case binary:match(Body, <<"\"entries\"">>) of
        nomatch -> [];
        {Start, _} ->
            Tail = binary:part(Body, Start, byte_size(Body) - Start),
            case binary:match(Tail, <<"[">>) of
                nomatch -> [];
                {ArrayStart, _} ->
                    Rest = binary:part(Tail, ArrayStart + 1, byte_size(Tail) - ArrayStart - 1),
                    parse_entry_array(Rest, [])
            end
    end.

parse_entry_array(<<>>, Acc) -> lists:reverse(Acc);
parse_entry_array(<<$], _Rest/binary>>, Acc) -> lists:reverse(Acc);
parse_entry_array(<<$,, Rest/binary>>, Acc) -> parse_entry_array(Rest, Acc);
parse_entry_array(<<$\s, Rest/binary>>, Acc) -> parse_entry_array(Rest, Acc);
parse_entry_array(<<$\n, Rest/binary>>, Acc) -> parse_entry_array(Rest, Acc);
parse_entry_array(<<$\r, Rest/binary>>, Acc) -> parse_entry_array(Rest, Acc);
parse_entry_array(<<$\t, Rest/binary>>, Acc) -> parse_entry_array(Rest, Acc);
parse_entry_array(<<${, Rest/binary>>, Acc) ->
    case extract_object(<<${, Rest/binary>>) of
        {ok, Obj, RestTail} ->
            case entry_pair(Obj) of
                {ok, Pair} -> parse_entry_array(RestTail, [Pair | Acc]);
                _ -> parse_entry_array(RestTail, Acc)
            end;
        _ -> lists:reverse(Acc)
    end;
parse_entry_array(_Bin, Acc) -> lists:reverse(Acc).

extract_object(<<${, Rest/binary>>) -> extract_object(Rest, 1, <<${>>).
extract_object(<<>>, _Depth, _Acc) -> error;
extract_object(<<${, Rest/binary>>, Depth, Acc) ->
    extract_object(Rest, Depth + 1, <<Acc/binary, ${>>);
extract_object(<<$}, Rest/binary>>, 1, Acc) ->
    {ok, <<Acc/binary, $}>>, Rest};
extract_object(<<$}, Rest/binary>>, Depth, Acc) ->
    extract_object(Rest, Depth - 1, <<Acc/binary, $}>>);
extract_object(<<$", Rest/binary>>, Depth, Acc) ->
    {Bin, Rest1} = consume_string(Rest, <<Acc/binary, $">>),
    extract_object(Rest1, Depth, Bin);
extract_object(<<C, Rest/binary>>, Depth, Acc) ->
    extract_object(Rest, Depth, <<Acc/binary, C>>).

consume_string(<<>>, Acc) -> {Acc, <<>>};
consume_string(<<$\\, $", Rest/binary>>, Acc) ->
    consume_string(Rest, <<Acc/binary, $\\, $">>);
consume_string(<<$", Rest/binary>>, Acc) ->
    {<<Acc/binary, $">>, Rest};
consume_string(<<C, Rest/binary>>, Acc) ->
    consume_string(Rest, <<Acc/binary, C>>).

entry_pair(Obj) ->
    case string_field(Obj, <<"\"key\"">>) of
        undefined -> error;
        Key ->
            ValueJson = extract_value_field(Obj),
            {ok, {Key, ValueJson}}
    end.

%% Returns the raw `"value"` field's JSON representation as a binary so the
%% original type (string / number / bool / array / object / null) survives
%% the round trip. Falls back to `null` if we can't isolate it.
extract_value_field(Obj) ->
    case binary:match(Obj, <<"\"value\"">>) of
        nomatch -> <<"null">>;
        {Start, KeyLen} ->
            Tail = binary:part(Obj, Start + KeyLen, byte_size(Obj) - Start - KeyLen),
            slice_json_value(skip_ws(skip_colon(skip_ws(Tail))))
    end.

skip_ws(<<$\s, Rest/binary>>) -> skip_ws(Rest);
skip_ws(<<$\n, Rest/binary>>) -> skip_ws(Rest);
skip_ws(<<$\r, Rest/binary>>) -> skip_ws(Rest);
skip_ws(<<$\t, Rest/binary>>) -> skip_ws(Rest);
skip_ws(Bin) -> Bin.

skip_colon(<<$:, Rest/binary>>) -> Rest;
skip_colon(Bin) -> Bin.

slice_json_value(<<$", Rest/binary>>) ->
    {Str, _} = consume_string(Rest, <<$">>),
    Str;
slice_json_value(<<${, _/binary>> = Bin) ->
    case extract_object(Bin) of
        {ok, Obj, _} -> Obj;
        _ -> <<"null">>
    end;
slice_json_value(<<$[, _/binary>> = Bin) ->
    slice_balanced(Bin, $[, $], 0, <<>>);
slice_json_value(<<>>) -> <<"null">>;
slice_json_value(Bin) ->
    slice_scalar(Bin, <<>>).

slice_balanced(<<>>, _Open, _Close, _Depth, Acc) -> Acc;
slice_balanced(<<Open, Rest/binary>>, Open, Close, Depth, Acc) ->
    slice_balanced(Rest, Open, Close, Depth + 1, <<Acc/binary, Open>>);
slice_balanced(<<Close, _Rest/binary>>, _Open, Close, 1, Acc) ->
    <<Acc/binary, Close>>;
slice_balanced(<<Close, Rest/binary>>, Open, Close, Depth, Acc) ->
    slice_balanced(Rest, Open, Close, Depth - 1, <<Acc/binary, Close>>);
slice_balanced(<<C, Rest/binary>>, Open, Close, Depth, Acc) ->
    slice_balanced(Rest, Open, Close, Depth, <<Acc/binary, C>>).

slice_scalar(<<>>, Acc) -> Acc;
slice_scalar(<<C, _Rest/binary>>, Acc) when C =:= $,; C =:= $}; C =:= $]; C =:= $\s; C =:= $\n; C =:= $\r; C =:= $\t ->
    Acc;
slice_scalar(<<C, Rest/binary>>, Acc) ->
    slice_scalar(Rest, <<Acc/binary, C>>).

%% ---------- environment + utils -----------------------------------------

env(Name) when is_binary(Name) ->
    case os:getenv(binary_to_list(Name)) of
        false -> undefined;
        "" -> undefined;
        Val -> list_to_binary(Val)
    end.

service_name() ->
    case env(<<"RUNTIME_CONFIG_SERVICE_NAME">>) of
        undefined -> <<"unknown">>;
        V -> V
    end.

scope_name() ->
    case env(<<"RUNTIME_CONFIG_SCOPE">>) of
        undefined -> service_name();
        V -> V
    end.

env_label() ->
    case env(<<"RUNTIME_CONFIG_ENV">>) of
        undefined -> <<"stage">>;
        V -> V
    end.

iso_now() ->
    {{Y, Mo, D}, {H, Mi, S}} = calendar:system_time_to_universal_time(
        erlang:system_time(second), second
    ),
    Bin = io_lib:format(
        "~4..0B-~2..0B-~2..0BT~2..0B:~2..0B:~2..0B.000Z",
        [Y, Mo, D, H, Mi, S]
    ),
    iolist_to_binary(Bin).

constant_time_equal(A, B) when is_binary(A), is_binary(B), byte_size(A) =:= byte_size(B) ->
    constant_time_equal(A, B, 0);
constant_time_equal(_, _) -> false.

constant_time_equal(<<>>, <<>>, Diff) -> Diff =:= 0;
constant_time_equal(<<A:8, RestA/binary>>, <<B:8, RestB/binary>>, Diff) ->
    constant_time_equal(RestA, RestB, Diff bor (A bxor B)).
