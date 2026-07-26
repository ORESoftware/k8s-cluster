%% Postgres authority for keyed durable serverless actors.
%%
%% Hot execution lives in dynamically supervised BEAM processes, but no
%% correctness depends on a process remaining alive. Every transaction first
%% acquires a short crash-expiring row lease, then commits state through both
%% the lease owner and state_version fence. All values enter SQL through psql
%% variables and :'name' literal quoting; no caller data is interpolated.
-module(lambda_actor_store).

-export([
    available/0,
    claim/4,
    commit/6,
    fail/5,
    get_state/2,
    list_due_alarms/1,
    reset/4,
    valid_actor_key/1,
    psql_input/1
]).

-define(PSQL_TIMEOUT_MS, 10000).
-define(MAX_OUTPUT_BYTES, 4194304).
-define(MAX_ERROR_BYTES, 8000).

available() ->
    case database_url() of
        {ok, _} -> true;
        _ -> false
    end.

claim(FunctionRef0, ActorKey0, Owner0, LeaseMs0) ->
    FunctionRef = to_binary(FunctionRef0),
    ActorKey = to_binary(ActorKey0),
    Owner = to_binary(Owner0),
    LeaseMs = clamp_int(LeaseMs0, 1000, 360000),
    case {identifier_clause(FunctionRef), valid_actor_key(ActorKey)} of
        {invalid, _} ->
            {error, <<"valid lambda function UUID or slug is required">>};
        {_, false} ->
            {error, <<"actor key contains unsupported characters">>};
        {Where, true} ->
            SelectSql = 'gleam_lambda_runner@pg_contract':lambda_functions_select_sql(),
            Selected = [
                "select * from (", SelectSql, ") as lambda_function_row ",
                "where ", Where,
                " and is_soft_deleted = false and status = 'active' limit 1"
            ],
            Definition = definition_object("c"),
            Sql = [
                "insert into lambda_actor_instances (function_id, actor_key) ",
                "select f.id::uuid, :'actor_key' from (", Selected, ") f ",
                "on conflict (function_id, actor_key) do nothing; ",
                "with selected as (", Selected, "), claimed as (",
                "update lambda_actor_instances a set ",
                "lease_owner = :'owner', ",
                "lease_until = clock_timestamp() + ",
                "(interval '1 millisecond' * :'lease_ms'::integer), ",
                "updated_at = now() ",
                "from selected f where a.function_id = f.id::uuid ",
                "and a.actor_key = :'actor_key' ",
                "and (a.lease_until is null or a.lease_until <= clock_timestamp()) ",
                "returning a.id::text as actor_id, a.actor_key, a.state, ",
                "a.state_version, a.alarm_at, f.*",
                ") select coalesce(",
                "(select jsonb_build_object(",
                "'status','claimed',",
                "'actorId', c.actor_id,",
                "'functionId', c.id,",
                "'actorKey', c.actor_key,",
                "'state', c.state,",
                "'stateVersion', c.state_version,",
                "'alarmAt', c.alarm_at,",
                "'definition', ", Definition,
                ")::text from claimed c),",
                "(select jsonb_build_object(",
                "'status','busy',",
                "'retryAfterMs', greatest(1, ceil(extract(epoch from ",
                "(a.lease_until - clock_timestamp())) * 1000)::bigint)",
                ")::text from lambda_actor_instances a ",
                "join selected f on f.id::uuid = a.function_id ",
                "where a.actor_key = :'actor_key' and a.lease_until > clock_timestamp()),",
                "'{\"status\":\"notFound\"}'",
                ")"
            ],
            Vars = [
                {"function_ref", FunctionRef},
                {"actor_key", ActorKey},
                {"owner", Owner},
                {"lease_ms", integer_to_binary(LeaseMs)}
            ],
            decode_object_result(run_psql(Vars, Sql))
    end.

commit(ActorId0, Owner0, Version0, State0, AlarmAt0, Kind0) ->
    ActorId = to_binary(ActorId0),
    Owner = to_binary(Owner0),
    Version = integer_value(Version0, -1),
    State = iolist_to_binary(json:encode(State0)),
    AlarmAt = nullable_binary(AlarmAt0),
    Kind = to_binary(Kind0),
    Sql = [
        "update lambda_actor_instances set ",
        "state = :'state'::jsonb, state_version = state_version + 1, ",
        "alarm_at = nullif(:'alarm_at', '')::timestamptz, ",
        "alarm_attempt = 0, lease_owner = null, lease_until = null, ",
        "last_invoked_at = now(), last_error = null, updated_at = now() ",
        "where id = :'actor_id'::uuid and lease_owner = :'owner' ",
        "and state_version = :'version'::bigint ",
        "returning jsonb_build_object(",
        "'id', id, 'key', actor_key, 'version', state_version, ",
        "'alarmAt', alarm_at, 'kind', :'kind'",
        ")::text"
    ],
    Vars = [
        {"actor_id", ActorId},
        {"owner", Owner},
        {"version", integer_to_binary(Version)},
        {"state", State},
        {"alarm_at", AlarmAt},
        {"kind", Kind}
    ],
    case decode_object_result(run_psql(Vars, Sql)) of
        {ok, Map} when map_size(Map) > 0 -> {ok, Map};
        {ok, _} -> {error, <<"actor lease or state-version fence was lost">>};
        {error, Reason} -> {error, Reason}
    end.

reset(ActorId0, Owner0, Version0, Kind0) ->
    commit(ActorId0, Owner0, Version0, #{}, null, Kind0).

fail(ActorId0, Owner0, Version0, Reason0, AlarmFailure) ->
    ActorId = to_binary(ActorId0),
    Owner = to_binary(Owner0),
    Version = integer_value(Version0, -1),
    Reason = clamp_binary(to_binary(Reason0), ?MAX_ERROR_BYTES),
    AlarmSql = case AlarmFailure of
        true ->
            [
                "alarm_at = case when alarm_attempt < 6 then ",
                "clock_timestamp() + (interval '1 second' * ",
                "(2 * power(2, alarm_attempt))) else null end, ",
                "alarm_attempt = least(6, alarm_attempt + 1), "
            ];
        false ->
            []
    end,
    Sql = [
        "update lambda_actor_instances set ",
        AlarmSql,
        "lease_owner = null, lease_until = null, ",
        "last_error = :'reason', updated_at = now() ",
        "where id = :'actor_id'::uuid and lease_owner = :'owner' ",
        "and state_version = :'version'::bigint returning id::text"
    ],
    Vars = [
        {"actor_id", ActorId},
        {"owner", Owner},
        {"version", integer_to_binary(Version)},
        {"reason", Reason}
    ],
    case run_psql(Vars, Sql) of
        {ok, <<>>} -> {error, <<"actor failure release lost its lease fence">>};
        {ok, _} -> ok;
        {error, StoreError} -> {error, StoreError}
    end.

get_state(FunctionRef0, ActorKey0) ->
    FunctionRef = to_binary(FunctionRef0),
    ActorKey = to_binary(ActorKey0),
    case {identifier_clause(FunctionRef), valid_actor_key(ActorKey)} of
        {invalid, _} ->
            {error, <<"valid lambda function UUID or slug is required">>};
        {_, false} ->
            {error, <<"actor key contains unsupported characters">>};
        {Where, true} ->
            Sql = [
                "select jsonb_build_object(",
                "'id', a.id, 'functionId', a.function_id, 'key', a.actor_key, ",
                "'state', a.state, 'version', a.state_version, ",
                "'alarmAt', a.alarm_at, 'alarmAttempt', a.alarm_attempt, ",
                "'lastInvokedAt', a.last_invoked_at, 'lastError', a.last_error, ",
                "'createdAt', a.created_at, 'updatedAt', a.updated_at",
                ")::text from lambda_actor_instances a ",
                "join lambda_functions f on f.id = a.function_id ",
                "where ", function_where("f", Where),
                " and a.actor_key = :'actor_key' ",
                "and f.is_soft_deleted = false limit 1"
            ],
            Vars = [
                {"function_ref", FunctionRef},
                {"actor_key", ActorKey}
            ],
            case decode_object_result(run_psql(Vars, Sql)) of
                {ok, Map} when map_size(Map) > 0 -> {ok, Map};
                {ok, _} -> {error, <<"durable actor not found">>};
                {error, Reason} -> {error, Reason}
            end
    end.

list_due_alarms(Limit0) ->
    Limit = clamp_int(Limit0, 1, 1000),
    Sql = [
        "select coalesce(jsonb_agg(jsonb_build_object(",
        "'functionId', due.function_id, 'actorKey', due.actor_key, ",
        "'alarmAt', due.alarm_at",
        ") order by due.alarm_at), '[]'::jsonb)::text from (",
        "select function_id, actor_key, alarm_at ",
        "from lambda_actor_instances where alarm_at <= clock_timestamp() ",
        "and (lease_until is null or lease_until <= clock_timestamp()) ",
        "order by alarm_at asc limit :'limit'::integer",
        ") due"
    ],
    case run_psql([{"limit", integer_to_binary(Limit)}], Sql) of
        {ok, Output} ->
            try json:decode(trim(Output)) of
                Values when is_list(Values) -> {ok, Values};
                _ -> {error, <<"actor alarm query returned invalid JSON">>}
            catch
                _:_ -> {error, <<"actor alarm query returned invalid JSON">>}
            end;
        {error, Reason} ->
            {error, Reason}
    end.

valid_actor_key(Value0) ->
    Value = to_binary(Value0),
    byte_size(Value) =< 200 andalso
        re:run(Value, "^[A-Za-z0-9][A-Za-z0-9._:-]{0,199}$", [{capture, none}]) =:= match.

definition_object(Alias) ->
    [
        "jsonb_build_object(",
        "'id', ", Alias, ".id,",
        "'slug', ", Alias, ".slug,",
        "'functionBody', ", Alias, ".function_body,",
        "'runtime', ", Alias, ".runtime,",
        "'entryCommand', ", Alias, ".entry_command,",
        "'reuseKey', ", Alias, ".reuse_key,",
        "'idleTimeoutSeconds', ", Alias, ".idle_timeout_seconds,",
        "'maxRunMs', ", Alias, ".max_run_ms,",
        "'containerized', ", Alias, ".containerized,",
        "'containerImage', ", Alias, ".container_image,",
        "'containerBuildStatus', ", Alias, ".container_build_status,",
        "'containerBuildError', ", Alias, ".container_build_error,",
        "'containerBuiltAt', ", Alias, ".container_built_at,",
        "'status', ", Alias, ".status,",
        "'labels', ", Alias, ".labels_json::jsonb,",
        "'metaData', ", Alias, ".meta_data_json::jsonb",
        ")"
    ].

identifier_clause(Identifier) ->
    case re:run(Identifier, "^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$", [{capture, none}]) of
        %% The canonical generated selector exposes UUID columns as text.
        match -> "id = :'function_ref'";
        nomatch ->
            case re:run(Identifier, "^[a-z0-9][a-z0-9-]{1,118}[a-z0-9]$", [{capture, none}]) of
                match -> "slug = :'function_ref'";
                nomatch -> invalid
            end
    end.

function_where(Alias, "id = :'function_ref'") ->
    [Alias, ".id = :'function_ref'::uuid"];
function_where(Alias, "slug = :'function_ref'") ->
    [Alias, ".slug = :'function_ref'"].

decode_object_result({ok, Output}) ->
    case trim(Output) of
        <<>> -> {ok, #{}};
        Json ->
            try json:decode(Json) of
                Map when is_map(Map) -> {ok, Map};
                _ -> {error, <<"actor query returned invalid JSON">>}
            catch
                _:_ -> {error, <<"actor query returned invalid JSON">>}
            end
    end;
decode_object_result({error, Reason}) ->
    {error, Reason}.

run_psql(Vars, Sql) ->
    case database_url() of
        {error, Reason} ->
            {error, Reason};
        {ok, DatabaseUrl} ->
            case os:find_executable("psql") of
                false ->
                    {error, <<"psql executable not found">>};
                Psql ->
                    VarArgs = lists:append([["-v", var_arg(Name, Value)] || {Name, Value} <- Vars]),
                    Args = VarArgs ++ [
                        "-X", "-q", "-At",
                        "-v", "ON_ERROR_STOP=1",
                        DatabaseUrl
                    ],
                    Port = open_port({spawn_executable, Psql}, [
                        binary, exit_status, stderr_to_stdout, use_stdio,
                        {args, Args}
                    ]),
                    true = port_command(Port, psql_input(Sql)),
                    collect_port(Port, [], 0)
            end
    end.

psql_input(Sql) ->
    iolist_to_binary([Sql, ";\n\\q\n"]).

var_arg(Name, Value) ->
    binary_to_list(iolist_to_binary([to_binary(Name), "=", to_binary(Value)])).

collect_port(Port, Chunks, Size) ->
    receive
        {Port, {data, Data}} ->
            NewSize = Size + byte_size(Data),
            case NewSize > ?MAX_OUTPUT_BYTES of
                true ->
                    close_port(Port),
                    {error, <<"actor query exceeded byte limit">>};
                false ->
                    collect_port(Port, [Data | Chunks], NewSize)
            end;
        {Port, {exit_status, 0}} ->
            {ok, iolist_to_binary(lists:reverse(Chunks))};
        {Port, {exit_status, Status}} ->
            Output = iolist_to_binary(lists:reverse(Chunks)),
            {error, iolist_to_binary(io_lib:format(
                "psql exited with status ~p: ~s",
                [Status, Output]
            ))}
    after ?PSQL_TIMEOUT_MS ->
        close_port(Port),
        {error, <<"actor query timed out">>}
    end.

close_port(Port) ->
    try erlang:port_close(Port) catch _:_ -> ok end.

database_url() ->
    case os:getenv("LAMBDA_DATABASE_URL") of
        false -> {error, <<"LAMBDA_DATABASE_URL is required">>};
        "" -> {error, <<"LAMBDA_DATABASE_URL is required">>};
        Value -> {ok, Value}
    end.

trim(Value) ->
    unicode:characters_to_binary(string:trim(binary_to_list(to_binary(Value)))).

nullable_binary(null) -> <<>>;
nullable_binary(undefined) -> <<>>;
nullable_binary(Value) -> to_binary(Value).

integer_value(Value, _Default) when is_integer(Value) -> Value;
integer_value(Value, Default) when is_binary(Value) ->
    case string:to_integer(binary_to_list(Value)) of
        {Integer, []} -> Integer;
        _ -> Default
    end;
integer_value(_, Default) -> Default.

clamp_int(Value, Min, _Max) when is_integer(Value), Value < Min -> Min;
clamp_int(Value, _Min, Max) when is_integer(Value), Value > Max -> Max;
clamp_int(Value, _Min, _Max) when is_integer(Value) -> Value;
clamp_int(_Value, Min, _Max) -> Min.

clamp_binary(Value, MaxBytes) when byte_size(Value) =< MaxBytes -> Value;
clamp_binary(Value, MaxBytes) -> binary:part(Value, 0, MaxBytes).

to_binary(Value) when is_binary(Value) -> Value;
to_binary(Value) when is_list(Value) -> unicode:characters_to_binary(Value);
to_binary(Value) -> unicode:characters_to_binary(io_lib:format("~p", [Value])).
