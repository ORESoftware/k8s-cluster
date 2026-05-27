%%% Merged Erlang FFI for `gleamlang-ws-server`.
%%%
%%% Combines the surfaces of the two predecessors:
%%%
%%%   1. legacy `gleamlang_server_env`
%%%        env / json id / now_ms / NATS-bridge HTTP publish
%%%
%%%   2. new `gleamlang_presence_server_ffi`
%%%        stable atom names, file IO, pgo config helpers,
%%%        consistent-shard hashing, wal2json parsing, test-only
%%%        named-process kill, self-node binary
%%%
%%% Function names from the two predecessors are disjoint apart from
%%% `getenv/1` ↔ `env/1`, both of which read an OS env var into a UTF-8
%%% binary. We keep both entry points: `env/1` is canonical, `getenv/1`
%%% is an alias so legacy Gleam call sites that already use
%%% `@external(erlang, _, "getenv")` keep working.
-module(gleamlang_ws_server_ffi).

-export([
    env/1,
    getenv/1,
    now_ms/0,
    json_message_id/1,
    publish_nats/1,
    stable_name/1,
    read_file_utf8/1,
    pgo_config/5,
    pgo_config_from_url/1,
    shard_of/2,
    self_node_binary/0,
    parse_wal2json/1,
    kill_named/1
]).

%% ─── env helpers ────────────────────────────────────────────────────────

%% Read an environment variable as a UTF-8 binary, returning `{ok, Value}`
%% or `{error, nil}`.
env(Name) when is_binary(Name) ->
    case os:getenv(binary_to_list(Name)) of
        false -> {error, nil};
        Value -> {ok, unicode:characters_to_binary(Value)}
    end.

%% Alias kept for backwards compatibility with the legacy
%% `gleamlang_server_env:getenv/1` call sites in `broadcaster.gleam`
%% and the merged `http_server.gleam`.
getenv(Name) -> env(Name).

now_ms() ->
    erlang:system_time(millisecond).

%% Best-effort JSON message-id extractor. We deliberately avoid pulling
%% in a JSON parser here; the broadcaster only needs to dedupe on a
%% string id, and a tight regex is fast enough for our payload sizes.
json_message_id(Payload) when is_binary(Payload) ->
    case re:run(
        Payload,
        <<"\"(?:messageId|message_id|id)\"\\s*:\\s*\"([^\"]{1,128})\"">>,
        [unicode, {capture, [1], binary}]
    ) of
        {match, [MessageId]} -> {ok, MessageId};
        _ -> {error, nil}
    end.

%% ─── NATS-bridge HTTP publish ───────────────────────────────────────────
%%
%% The legacy single-pod deployment runs a Node.js `nats-bridge` sidecar
%% on `127.0.0.1:8083`. WebSocket frames inbound to `/ws` (broadcaster
%% mode) are forwarded over localhost HTTP so the sidecar can `nats.publish`
%% them on `dd.remote.websocket.events`. The presence service instead
%% talks NATS directly via `dd_nats.erl`, but we keep this helper for
%% deployments that still use the sidecar.
publish_nats(Payload) when is_binary(Payload) ->
    case os:getenv("GLEAM_BROADCAST_SECRET") of
        false ->
            {error, nil};
        "" ->
            {error, nil};
        Secret ->
            Url = os:getenv("GLEAM_NATS_PUBLISH_URL", "http://127.0.0.1:8083/publish"),
            %% NATS_PUBLISH_SUBJECT default comes from dd_nats_subject_consts
            %% (auto-generated from remote/libs/nats/subject-defs/schema/
            %% runtime-events.schema.json) so a schema rename surfaces at
            %% build time instead of silently drifting between Erlang FFI
            %% and the rest of the codebase.
            Subject = case os:getenv("NATS_PUBLISH_SUBJECT") of
                false -> binary_to_list(dd_nats_subject_consts:websocket_events_subject());
                "" -> binary_to_list(dd_nats_subject_consts:websocket_events_subject());
                Override -> Override
            end,
            spawn(fun() -> post_nats_publish(Url, Secret, Subject, Payload) end),
            {ok, nil}
    end.

post_nats_publish(Url, Secret, Subject, Payload) ->
    case parse_http_url(Url) of
        {ok, Host, Port, Path} ->
            case gen_tcp:connect(Host, Port, [binary, {active, false}], 2000) of
                {ok, Socket} ->
                    Request = [
                        "POST ", Path, " HTTP/1.1\r\n",
                        "Host: ", Host, ":", integer_to_list(Port), "\r\n",
                        "Connection: close\r\n",
                        "Content-Type: text/plain; charset=utf-8\r\n",
                        "Content-Length: ", integer_to_list(byte_size(Payload)), "\r\n",
                        "x-dd-internal-auth: ", Secret, "\r\n",
                        "x-nats-subject: ", Subject, "\r\n",
                        "\r\n",
                        Payload
                    ],
                    ok = gen_tcp:send(Socket, Request),
                    case recv_status(Socket) of
                        {ok, Status} when Status >= 200, Status < 300 ->
                            ok;
                        {ok, Status} ->
                            io:format("ws-server nats publish failed status=~p~n", [Status]);
                        {error, Reason} ->
                            io:format("ws-server nats publish response failed: ~p~n", [Reason])
                    end,
                    gen_tcp:close(Socket);
                {error, Reason} ->
                    io:format("ws-server nats publish connect failed: ~p~n", [Reason])
            end;
        {error, Reason} ->
            io:format("ws-server nats publish invalid url: ~p~n", [Reason])
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
            {error, Url}
    catch
        _:_ -> {error, Url}
    end.

recv_status(Socket) ->
    case gen_tcp:recv(Socket, 0, 2000) of
        {ok, Response} ->
            case binary:split(Response, <<"\r\n">>) of
                [StatusLine | _] ->
                    case binary:split(StatusLine, <<" ">>, [global]) of
                        [_Http, StatusBin | _] ->
                            try {ok, binary_to_integer(StatusBin)}
                            catch _:_ -> {error, StatusLine}
                            end;
                        _ ->
                            {error, StatusLine}
                    end;
                _ ->
                    {error, Response}
            end;
        {error, Reason} ->
            {error, Reason}
    end.

%% ─── presence-server helpers ────────────────────────────────────────────

%% Build a process Name (which is just an Erlang atom internally) from a
%% known string. Unlike `gleam_erlang_ffi:new_name/1` this does NOT append
%% a unique suffix, so two BEAM nodes that both call
%% `stable_name(<<"presence_fanout_relay">>)` end up with the SAME atom and
%% therefore the SAME `Name(msg)` value.
stable_name(S) ->
    erlang:binary_to_atom(S, utf8).

%% Read a small file (a few KB) into a UTF-8 binary. Used to read the
%% in-pod k8s service account token (~1 KB JWT). Returns `{ok, Body}` or
%% `{error, ReasonBinary}`.
read_file_utf8(Path) ->
    case file:read_file(Path) of
        {ok, Body} -> {ok, Body};
        {error, Reason} ->
            ReasonBin =
                list_to_binary(io_lib:format("~p", [Reason])),
            {error, ReasonBin}
    end.

%% Build a pgo:pool_config() map from explicit fields.
pgo_config(Host, Port, User, Password, Database) ->
    #{host => binary_to_list(Host),
      port => Port,
      user => binary_to_list(User),
      password => binary_to_list(Password),
      database => binary_to_list(Database),
      pool_size => 1}.

%% Parse a `postgres://[user[:pass]@]host[:port]/database` URL into a
%% pgo:pool_config() map.
pgo_config_from_url(Url) ->
    UrlStr = binary_to_list(Url),
    try
        Scheme =
            case lists:prefix("postgres://", UrlStr) of
                true -> "postgres://";
                false ->
                    case lists:prefix("postgresql://", UrlStr) of
                        true -> "postgresql://";
                        false -> throw({bad_scheme, UrlStr})
                    end
            end,
        Body = lists:nthtail(length(Scheme), UrlStr),
        {Auth, HostPart} =
            case string:split(Body, "@") of
                [HostOnly] -> {"", HostOnly};
                [A, H] -> {A, H};
                _ -> throw(bad_auth)
            end,
        {User, Pass} =
            case Auth of
                "" -> {"", ""};
                _ ->
                    case string:split(Auth, ":") of
                        [U] -> {U, ""};
                        [U, P] -> {U, P}
                    end
            end,
        {HostPort, DbPart} =
            case string:split(HostPart, "/") of
                [HP, D0] -> {HP, D0};
                _ -> throw(bad_path)
            end,
        {Host, Port} =
            case string:split(HostPort, ":") of
                [HostOnly2] -> {HostOnly2, 5432};
                [HostA, PStr] -> {HostA, list_to_integer(PStr)}
            end,
        Database =
            case string:split(DbPart, "?") of
                [DOnly] -> DOnly;
                [DBefore, _Q] -> DBefore
            end,
        Cfg = #{host => Host,
                port => Port,
                user => User,
                password => Pass,
                database => Database,
                pool_size => 1},
        {ok, Cfg}
    catch
        throw:Reason ->
            ReasonBin =
                list_to_binary(io_lib:format("invalid PG URL: ~p", [Reason])),
            {error, ReasonBin};
        Class:Reason:_St ->
            ReasonBin =
                list_to_binary(io_lib:format("~p:~p", [Class, Reason])),
            {error, ReasonBin}
    end.

%% Compute the shard a slug or UUID maps to. Must match Postgres'
%% algorithm in `notify_presence_member_change()` exactly.
shard_of(Id, NShards0) ->
    NShards =
        case NShards0 of
            N when is_integer(N), N > 0 -> N;
            _ -> 256
        end,
    Bin = case Id of
              B when is_binary(B) -> B;
              L when is_list(L) -> list_to_binary(L)
          end,
    Hex = case is_uuid_text(Bin) of
              true ->
                  binary:replace(Bin, <<"-">>, <<>>, [global]);
              false ->
                  hex_encode(erlang:md5(Bin))
          end,
    case Hex of
        <<H1, H2, H3, H4, _/binary>> ->
            HexPrefix = <<H1, H2, H3, H4>>,
            try binary_to_integer(HexPrefix, 16) of
                Int -> Int rem NShards
            catch
                error:badarg ->
                    erlang:phash2(Id, NShards)
            end;
        _ ->
            erlang:phash2(Id, NShards)
    end.

is_uuid_text(<<A:8/binary, "-", B:4/binary, "-", C:4/binary, "-",
               D:4/binary, "-", E:12/binary>>) ->
    is_hex(A) andalso is_hex(B) andalso is_hex(C)
    andalso is_hex(D) andalso is_hex(E);
is_uuid_text(_) -> false.

is_hex(<<>>) -> true;
is_hex(<<C, Rest/binary>>) when (C >= $0 andalso C =< $9)
                                ; (C >= $a andalso C =< $f)
                                ; (C >= $A andalso C =< $F) ->
    is_hex(Rest);
is_hex(_) -> false.

hex_encode(Bin) ->
    list_to_binary([io_lib:format("~2.16.0b", [B]) || <<B>> <= Bin]).

self_node_binary() ->
    atom_to_binary(node(), utf8).

%% Parse a wal2json v2 line for a `presence_conv_members` change.
parse_wal2json(Json) ->
    try
        Decoded = decode_json(Json),
        case Decoded of
            #{<<"action">> := Action} = Obj
              when Action =:= <<"I">>; Action =:= <<"U">>; Action =:= <<"D">> ->
                Columns = maps:get(<<"columns">>, Obj, []),
                Identity = maps:get(<<"identity">>, Obj, []),
                Source =
                    case Action of
                        <<"D">> -> Identity;
                        _ -> Columns
                    end,
                ConvId = lookup_col(<<"conv_id">>, Source),
                UserId = lookup_col(<<"user_id">>, Source),
                Soft = lookup_col(<<"is_soft_deleted">>, Columns),
                case {ConvId, UserId} of
                    {undefined, _} -> {error, missing_conv};
                    {_, undefined} -> {error, missing_user};
                    {C, U} ->
                        SoftBool =
                            case Soft of
                                true -> true;
                                <<"true">> -> true;
                                _ -> false
                            end,
                        {ok, {Action, to_binary(C), to_binary(U), SoftBool}}
                end;
            _ ->
                {error, skip}
        end
    catch
        Class:Reason:_St ->
            ReasonBin =
                list_to_binary(io_lib:format("~p:~p", [Class, Reason])),
            {error, ReasonBin}
    end.

decode_json(Bin) when is_binary(Bin) ->
    json:decode(Bin).

lookup_col(_Name, []) -> undefined;
lookup_col(Name, [#{<<"name">> := Name, <<"value">> := V} | _]) -> V;
lookup_col(Name, [_ | Rest]) -> lookup_col(Name, Rest).

to_binary(B) when is_binary(B) -> B;
to_binary(L) when is_list(L) -> list_to_binary(L);
to_binary(I) when is_integer(I) -> integer_to_binary(I);
to_binary(N) when is_atom(N) -> atom_to_binary(N, utf8).

%% Test-only helper: kill a registered process synchronously so a fresh
%% `register/2` (or `actor.named`) under the same atom can succeed.
kill_named(NameBin) ->
    Name = binary_to_atom(NameBin, utf8),
    case erlang:whereis(Name) of
        undefined -> nil;
        Pid when is_pid(Pid) ->
            erlang:unlink(Pid),
            MRef = erlang:monitor(process, Pid),
            erlang:exit(Pid, kill),
            receive
                {'DOWN', MRef, process, Pid, _} -> nil
            after 1000 ->
                erlang:demonitor(MRef, [flush]),
                nil
            end
    end.
