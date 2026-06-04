-module(gleamlang_presence_server_ffi).

-export([
    stable_name/1,
    env/1,
    read_file_utf8/1,
    pgo_config/5,
    pgo_config_from_url/1,
    shard_of/2,
    self_node_binary/0,
    parse_wal2json/1,
    kill_named/1
]).

%% Build a process Name (which is just an Erlang atom internally) from a
%% known string. Unlike `gleam_erlang_ffi:new_name/1` this does NOT append
%% a unique suffix, so two BEAM nodes that both call
%% `stable_name(<<"presence_fanout_relay">>)` end up with the SAME atom and
%% therefore the SAME `Name(msg)` value.
stable_name(S) ->
    erlang:binary_to_atom(S, utf8).

%% Read an environment variable as a UTF-8 binary, returning `{ok, Value}`
%% or `{error, nil}`. The Gleam side decodes the result via a small wrapper.
env(Name) ->
    case os:getenv(binary_to_list(Name)) of
        false -> {error, nil};
        Value -> {ok, list_to_binary(Value)}
    end.

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

%% Build a pgo:pool_config() map from explicit fields. The pgo library
%% expects strings, not binaries, for the connection params.
pgo_config(Host, Port, User, Password, Database) ->
    #{host => binary_to_list(Host),
      port => Port,
      user => binary_to_list(User),
      password => binary_to_list(Password),
      database => binary_to_list(Database),
      %% No idle pool needed for pgo_notifications — it owns its own
      %% dedicated socket and we want it to live as long as the process.
      pool_size => 1}.

%% Parse a `postgres://[user[:pass]@]host[:port]/database` URL into a
%% pgo:pool_config() map. Minimal parser — handles the common forms used
%% by env vars (PG_DATABASE_URL). Returns {ok, Map} or {error, Reason}.
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
        %% Database may contain a query string. Drop it.
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
%% algorithm in `notify_presence_member_change()` (schema.sql) exactly
%% — otherwise pg_listen subscribes to the wrong channel.
%%
%% Algorithm:
%%   1. If the input looks like a canonical UUID, take the first 16
%%      bits of the hex form and modulo N.
%%   2. Otherwise compute md5(input) and use the first 16 bits of
%%      THAT as the shard input. This mirrors PG's
%%      `presence_to_uuid(text)` which md5's non-UUID strings before
%%      casting to uuid — so both sides land on the same shard for
%%      demo IDs ("conv-1", "alice") as well as real UUIDs.
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
    %% Hex form to shard against — UUIDs passed through, slugs md5'd.
    Hex = case is_uuid_text(Bin) of
              true ->
                  binary:replace(Bin, <<"-">>, <<>>, [global]);
              false ->
                  %% md5/1 returns a 16-byte binary, the hex encoding of
                  %% which mirrors what `md5(p)::uuid` produces in PG.
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
    %% Convert <<H1, H2, ...>> bytes to <<"h1h2...">> lowercase hex.
    list_to_binary([io_lib:format("~2.16.0b", [B]) || <<B>> <= Bin]).

%% Erlang short/long node name as a binary, for use as a NATS Source-Node
%% header and for log lines.
self_node_binary() ->
    atom_to_binary(node(), utf8).

%% Parse a wal2json v2 line for a `presence_conv_members` change, returning
%% `{ok, {Action, ConvId, UserId, SoftDeleted}}` or `{error, Reason}`.
%%
%% The wal2json v2 format emits one JSON object per row change with:
%%   {"action":"I"|"U"|"D",
%%    "schema":"public",
%%    "table":"presence_conv_members",
%%    "columns":[{"name":"...","value":...}, ...],
%%    "identity":[...]}             %% for U / D
%%
%% We pull `conv_id` and `user_id` from `columns` (INSERT/UPDATE) or
%% `identity` (DELETE), and `is_soft_deleted` from `columns` if present.
%% Non-row messages (BEGIN, COMMIT, TRUNCATE, MESSAGE) are returned as
%% `{error, skip}` so the Gleam caller can drop them silently.
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
                %% BEGIN/COMMIT/TRUNCATE/MESSAGE/etc. — not interesting.
                {error, skip}
        end
    catch
        Class:Reason:_St ->
            ReasonBin =
                list_to_binary(io_lib:format("~p:~p", [Class, Reason])),
            {error, ReasonBin}
    end.

decode_json(Bin) when is_binary(Bin) ->
    %% OTP 27+ ships a built-in json module. Falls back gracefully if
    %% unavailable, though all our target releases include it.
    json:decode(Bin).

lookup_col(_Name, []) -> undefined;
lookup_col(Name, [#{<<"name">> := Name, <<"value">> := V} | _]) -> V;
lookup_col(Name, [_ | Rest]) -> lookup_col(Name, Rest).

to_binary(B) when is_binary(B) -> B;
to_binary(L) when is_list(L) -> list_to_binary(L);
to_binary(I) when is_integer(I) -> integer_to_binary(I);
to_binary(N) when is_atom(N) -> atom_to_binary(N, utf8).

%% Test-only helper: if `Name` is a registered process, kill it
%% synchronously and wait until the registration is released, so a
%% subsequent `register/2` (or `actor.named`) under the same atom can
%% succeed.
%%
%% We `unlink/1` first because `actor.start` links the actor to the
%% spawning test process; an `exit(Pid, kill)` on a linked process would
%% propagate the kill signal back to the test runner via the link.
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
