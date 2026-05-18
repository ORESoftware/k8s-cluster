-module(gleamlang_presence_server_ffi).

-export([
    stable_name/1,
    env/1,
    read_file_utf8/1,
    pgo_config/5,
    pgo_config_from_url/1,
    shard_of/2,
    self_node_binary/0,
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

%% Compute the shard a conv_id maps to. Must match Postgres' algorithm
%% in `notify_presence_member_change()` (schema.sql) exactly — otherwise
%% pg_listen subscribes to the wrong channel and misses NOTIFYs.
%%
%% Algorithm: take the first 16 bits of the canonical UUID hex form
%% (after removing hyphens), interpret as an unsigned int, modulo N.
%% Deterministic across PG / BEAM / any other client.
shard_of(ConvId, NShards) ->
    Bin = case ConvId of
              B when is_binary(B) -> B;
              L when is_list(L) -> list_to_binary(L)
          end,
    Hex = binary:replace(Bin, <<"-">>, <<>>, [global]),
    case Hex of
        <<H1, H2, H3, H4, _/binary>> ->
            HexPrefix = <<H1, H2, H3, H4>>,
            try binary_to_integer(HexPrefix, 16) of
                Int -> Int rem NShards
            catch
                error:badarg ->
                    %% Non-hex prefix (e.g., demo IDs like "conv-1").
                    %% Fall back to phash2 so the listener doesn't crash.
                    %% PG-side trigger will also bail (its substring/cast
                    %% raises), so any cross-side disagreement here is
                    %% moot — we simply never receive a notification for
                    %% non-UUID conv_ids.
                    erlang:phash2(ConvId, NShards)
            end;
        _ ->
            erlang:phash2(ConvId, NShards)
    end.

%% Erlang short/long node name as a binary, for use as a NATS Source-Node
%% header and for log lines.
self_node_binary() ->
    atom_to_binary(node(), utf8).

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
