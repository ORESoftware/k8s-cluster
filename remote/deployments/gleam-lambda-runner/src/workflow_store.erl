%% Postgres persistence for the workflow execution engine.
%%
%% Mirrors lambda_child_runner's psql-subprocess access pattern (no native PG
%% driver in this service). All caller-supplied values are bound through psql
%% `-v name=value` variables and referenced with `:'name'` colon-quoting, which
%% emits a properly escaped SQL string literal. open_port/2 passes argv elements
%% literally (no shell), so there is no shell-injection surface either.
%%
%% Durable state lives in workflow_runs / workflow_step_runs (see
%% remote/libs/pg-defs/schema/schema.sql). This module is the only writer; the
%% engine computes transitions and calls the typed functions below.
-module(workflow_store).

-export([
    available/0,
    create_run/3,
    get_run/1,
    get_run_with_steps/1,
    list_runs/2,
    claim_due/2,
    succeed_advance/4,
    succeed_complete/4,
    fail_retry/5,
    fail_terminal/3,
    enter_sleep/4,
    park_wait/2,
    consume_signal/5,
    repark_wait/1,
    cancel_run/1,
    deliver_signal/3,
    %% Exported for unit tests (pure helper).
    clamp_text/2
]).

-define(PSQL_TIMEOUT_MS, 5000).
%% Claim batches embed each run's definition steps, so the response can be
%% several MB for large definitions; keep a generous but bounded read cap.
-define(MAX_OUTPUT_BYTES, 16777216).

%% Column length limits from schema.sql. Oversized values are truncated here so
%% an INSERT/UPDATE can never fail-loop a run on a constraint violation.
-define(MAX_NAME_LEN, 200).
-define(MAX_TYPE_LEN, 32).
-define(MAX_REF_LEN, 200).
-define(MAX_ERROR_LEN, 8000).
-define(MAX_KEY_LEN, 200).
-define(MAX_SIGNALS, 256).

%% Non-terminal statuses a transition is allowed to move away from. We never
%% resurrect completed/failed/canceled runs, so every guarded UPDATE filters on
%% this set; a 0-row result means the run was concurrently cancelled/changed.
-define(LIVE_STATUSES, "'pending','running','sleeping','waiting'").

available() ->
    case database_url() of
        {ok, _} -> true;
        _ -> false
    end.

%% ─── Reads ──────────────────────────────────────────────────────────────────

%% Resolve an active definition by UUID or slug, then insert a pending run.
%% Honours per-definition idempotency: a repeat (definition_id, idempotency_key)
%% returns the existing run instead of creating a duplicate.
create_run(DefRef0, InputJson0, IdempotencyKey0) ->
    DefRef = to_binary(DefRef0),
    InputJson = normalize_json(to_binary(InputJson0), <<"null">>),
    IdempotencyKey = to_binary(IdempotencyKey0),
    case resolve_definition(DefRef) of
        {ok, DefId} ->
            insert_run(DefId, InputJson, IdempotencyKey);
        {error, Reason} ->
            {error, Reason}
    end.

resolve_definition(DefRef) ->
    Where =
        case identifier_kind(DefRef) of
            uuid -> "id = :'ref'::uuid";
            slug -> "slug = :'ref'";
            invalid -> invalid
        end,
    case Where of
        invalid ->
            {error, <<"valid workflow definition UUID or slug is required">>};
        Clause ->
            Sql = [
                "select id::text from workflow_definitions where ",
                Clause,
                " and is_soft_deleted = false and status = 'active' limit 1"
            ],
            case run_psql([{"ref", DefRef}], Sql) of
                {ok, <<>>} -> {error, <<"workflow definition not found or not active">>};
                {ok, Id} -> {ok, string:trim(Id)};
                {error, Reason} -> {error, Reason}
            end
    end.

insert_run(DefId, InputJson, IdempotencyKey0) ->
    IdempotencyKey = clamp_text(IdempotencyKey0, ?MAX_KEY_LEN),
    {IdemExpr, IdemVars} =
        case IdempotencyKey of
            <<>> -> {"null", []};
            _ -> {":'idem'", [{"idem", IdempotencyKey}]}
        end,
    Sql = [
        "insert into workflow_runs ",
        "(definition_id, definition_slug, status, input, wake_at, idempotency_key) ",
        "select d.id, d.slug, 'pending', :'input'::jsonb, now(), ", IdemExpr, " ",
        "from workflow_definitions d where d.id = :'def'::uuid ",
        "on conflict (definition_id, idempotency_key) where idempotency_key is not null ",
        "do nothing ",
        "returning ", run_json("workflow_runs")
    ],
    Vars = [{"def", DefId}, {"input", InputJson}] ++ IdemVars,
    case run_psql(Vars, Sql) of
        {ok, <<>>} when IdempotencyKey =/= <<>> ->
            %% Idempotency conflict: return the already-created run.
            existing_idempotent_run(DefId, IdempotencyKey);
        {ok, <<>>} ->
            {error, <<"failed to create workflow run">>};
        {ok, Json} ->
            {ok, string:trim(Json)};
        {error, Reason} ->
            {error, Reason}
    end.

existing_idempotent_run(DefId, IdempotencyKey) ->
    Sql = [
        "select ", run_json("workflow_runs"),
        " from workflow_runs where definition_id = :'def'::uuid ",
        "and idempotency_key = :'idem' limit 1"
    ],
    case run_psql([{"def", DefId}, {"idem", IdempotencyKey}], Sql) of
        {ok, <<>>} -> {error, <<"failed to create workflow run">>};
        {ok, Json} -> {ok, string:trim(Json)};
        {error, Reason} -> {error, Reason}
    end.

get_run(RunId0) ->
    RunId = to_binary(RunId0),
    case identifier_kind(RunId) of
        uuid ->
            Sql = [
                "select ", run_json("workflow_runs"),
                " from workflow_runs where id = :'rid'::uuid limit 1"
            ],
            case run_psql([{"rid", RunId}], Sql) of
                {ok, <<>>} -> {error, <<"workflow run not found">>};
                {ok, Json} -> {ok, string:trim(Json)};
                {error, Reason} -> {error, Reason}
            end;
        _ ->
            {error, <<"valid workflow run UUID is required">>}
    end.

%% Run plus its step-run history, for the GET endpoint.
get_run_with_steps(RunId0) ->
    RunId = to_binary(RunId0),
    case identifier_kind(RunId) of
        uuid ->
            Sql = [
                "select jsonb_build_object(",
                "'ok', true,",
                "'run', (select ", run_json("workflow_runs"),
                " from workflow_runs where id = :'rid'::uuid),",
                "'steps', coalesce((select jsonb_agg(s order by s.step_index, s.attempt) ",
                "from (select step_index, step_name, step_type, function_ref, attempt, status, ",
                "input, output, error, duration_ms, started_at, finished_at ",
                "from workflow_step_runs where run_id = :'rid'::uuid) s), '[]'::jsonb)",
                ")::text"
            ],
            case run_psql([{"rid", RunId}], Sql) of
                {ok, <<>>} -> {error, <<"workflow run not found">>};
                {ok, Json} -> {ok, string:trim(Json)};
                {error, Reason} -> {error, Reason}
            end;
        _ ->
            {error, <<"valid workflow run UUID is required">>}
    end.

list_runs(DefRef0, Limit0) ->
    Limit = clamp_int(Limit0, 1, 500, 100),
    DefRef = to_binary(DefRef0),
    {Where, Vars} =
        case DefRef of
            <<>> ->
                {"", []};
            _ ->
                case identifier_kind(DefRef) of
                    uuid -> {" where definition_id = :'ref'::uuid", [{"ref", DefRef}]};
                    _ -> {" where definition_slug = :'ref'", [{"ref", DefRef}]}
                end
        end,
    Sql = [
        "select coalesce(jsonb_agg(o order by created_at desc), '[]'::jsonb)::text from (",
        "select ", run_json_object("workflow_runs"), " as o, created_at ",
        "from workflow_runs", Where, " order by created_at desc limit ", integer_to_list(Limit),
        ") rows"
    ],
    case run_psql(Vars, Sql) of
        {ok, <<>>} -> {ok, <<"[]">>};
        {ok, Json} -> {ok, string:trim(Json)};
        {error, Reason} -> {error, Reason}
    end.

%% ─── Scheduler claim ────────────────────────────────────────────────────────

%% Atomically lease up to Limit due runs (FOR UPDATE SKIP LOCKED) without
%% changing their semantic status, and return them decoded with the embedded
%% definition steps plus DB-clock fields the engine needs (nowMs/waitDeadlineMs).
claim_due(Limit0, LeaseMs0) ->
    Limit = clamp_int(Limit0, 1, 200, 25),
    LeaseMs = clamp_int(LeaseMs0, 1000, 600000, 60000),
    Sql = [
        "with due as (",
        "  select id from workflow_runs",
        "  where status in (", ?LIVE_STATUSES, ")",
        "    and wake_at is not null and wake_at <= now()",
        "    and (lease_until is null or lease_until <= now())",
        "  order by wake_at asc limit ", integer_to_list(Limit),
        "  for update skip locked",
        "), upd as (",
        "  update workflow_runs r set lease_until = now() + (:'lease' || ' milliseconds')::interval,",
        "    updated_at = now() from due where r.id = due.id returning r.*",
        ") select coalesce(jsonb_agg(jsonb_build_object(",
        "  'id', u.id, 'definitionId', u.definition_id, 'definitionSlug', u.definition_slug,",
        "  'status', u.status, 'currentStepIndex', u.current_step_index, 'attempt', u.attempt,",
        "  'input', u.input, 'context', u.context, 'signals', u.signals,",
        "  'waitDeadlineMs', case when u.wait_deadline is null then null",
        "     else (extract(epoch from u.wait_deadline) * 1000)::bigint end,",
        "  'nowMs', (extract(epoch from now()) * 1000)::bigint,",
        "  'steps', d.steps, 'defaultRetry', d.default_retry",
        ")), '[]'::jsonb)::text ",
        "from upd u join workflow_definitions d on d.id = u.definition_id"
    ],
    case run_psql([{"lease", integer_to_binary(LeaseMs)}], Sql) of
        {ok, <<>>} -> {ok, []};
        {ok, Json} -> {ok, decode_run_list(Json)};
        {error, Reason} -> {error, Reason}
    end.

decode_run_list(Json) ->
    try json:decode(iolist_to_binary(string:trim(Json))) of
        List when is_list(List) -> List;
        _ -> []
    catch
        _:_ -> []
    end.

%% ─── Transitions ────────────────────────────────────────────────────────────

%% Advance to the next step after a successful activity.
succeed_advance(RunId, StepRow, NextIndex, NewContextJson) ->
    Set = [
        set_int("current_step_index", NextIndex),
        set_int("attempt", 0),
        set_json("context", NewContextJson),
        {"status = 'running'", []},
        {"wake_at = now()", []},
        {"last_error = null", []}
    ],
    commit(RunId, Set, StepRow).

%% Final step succeeded: complete the run.
succeed_complete(RunId, StepRow, FinalOutputJson, NewContextJson) ->
    Set = [
        set_json("context", NewContextJson),
        set_json("output", FinalOutputJson),
        {"status = 'completed'", []},
        {"finished_at = now()", []},
        {"wake_at = null", []},
        {"last_error = null", []}
    ],
    commit(RunId, Set, StepRow).

%% Activity failed but has retries left: schedule a backoff retry.
fail_retry(RunId, StepRow, NewAttempt, BackoffMs, ErrBin) ->
    Set = [
        set_int("attempt", NewAttempt),
        set_text("last_error", ErrBin),
        {"status = 'running'", []},
        set_interval("wake_at", BackoffMs)
    ],
    commit(RunId, Set, StepRow).

%% Activity failed and retries are exhausted (or step is non-retryable).
fail_terminal(RunId, StepRow, ErrBin) ->
    Set = [
        set_text("last_error", ErrBin),
        {"status = 'failed'", []},
        {"finished_at = now()", []},
        {"wake_at = null", []}
    ],
    commit(RunId, Set, StepRow).

%% Durable timer: advance past the sleep step and park until the timer fires.
enter_sleep(RunId, StepRow, DurationMs, NextIndex) ->
    Set = [
        set_int("current_step_index", NextIndex),
        set_int("attempt", 0),
        {"status = 'sleeping'", []},
        set_interval("wake_at", DurationMs)
    ],
    commit(RunId, Set, StepRow).

%% Block on an external signal. DeadlineMs = infinity parks with no timeout
%% (wake_at null), so the scheduler only revisits when a signal sets wake_at.
park_wait(RunId, infinity) ->
    Set = [
        {"status = 'waiting'", []},
        {"wait_deadline = null", []},
        {"wake_at = null", []}
    ],
    commit(RunId, Set, undefined);
park_wait(RunId, DeadlineMs) when is_integer(DeadlineMs) ->
    Set = [
        {"status = 'waiting'", []},
        set_interval("wait_deadline", DeadlineMs),
        set_interval("wake_at", DeadlineMs)
    ],
    commit(RunId, Set, undefined).

%% A matching signal arrived: drop it from the queue and advance.
consume_signal(RunId, StepRow, NextIndex, NewContextJson, ConsumeIndex) ->
    Set = [
        set_int("current_step_index", NextIndex),
        set_int("attempt", 0),
        set_json("context", NewContextJson),
        {"status = 'running'", []},
        {"wait_deadline = null", []},
        {"wake_at = now()", []},
        {"signals = signals - " ++ integer_to_list(ConsumeIndex), []}
    ],
    commit(RunId, Set, StepRow).

%% Woken (e.g. by a non-matching signal) before the wait deadline: re-park,
%% keeping the original deadline so the timeout still fires on schedule.
repark_wait(RunId) ->
    Set = [{"wake_at = wait_deadline", []}],
    commit(RunId, Set, undefined).

cancel_run(RunId0) ->
    RunId = to_binary(RunId0),
    case identifier_kind(RunId) of
        uuid ->
            Set = [
                {"status = 'canceled'", []},
                {"finished_at = now()", []},
                {"wake_at = null", []},
                {"lease_until = null", []}
            ],
            commit(RunId, Set, undefined);
        _ ->
            {error, <<"valid workflow run UUID is required">>}
    end.

deliver_signal(RunId0, NameJson0, PayloadJson0) ->
    RunId = to_binary(RunId0),
    case identifier_kind(RunId) of
        uuid ->
            NameJson = normalize_json(to_binary(NameJson0), <<"\"\"">>),
            PayloadJson = normalize_json(to_binary(PayloadJson0), <<"null">>),
            %% Bound the queue so a run waiting on a signal that never arrives
            %% can't be grown without limit by signal spam: when already at the
            %% cap, drop the oldest element (jsonb `- 0`) before appending. This
            %% avoids any correlated-subquery semantics and is plain jsonb ops.
            NewElem =
                "jsonb_build_array(jsonb_build_object("
                "'name', :'name'::jsonb, 'payload', :'payload'::jsonb, "
                "'at', to_char(now() at time zone 'utc', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')))",
            Sql = [
                "update workflow_runs set ",
                "signals = case when jsonb_array_length(signals) >= ", integer_to_list(?MAX_SIGNALS),
                " then (signals - 0) || ", NewElem,
                " else signals || ", NewElem, " end,",
                "wake_at = now(), updated_at = now() ",
                "where id = :'rid'::uuid and status in (", ?LIVE_STATUSES, ") ",
                "returning ", run_json("workflow_runs")
            ],
            Vars = [{"rid", RunId}, {"name", NameJson}, {"payload", PayloadJson}],
            case run_psql(Vars, Sql) of
                {ok, <<>>} -> {error, <<"workflow run not running">>};
                {ok, Json} -> {ok, string:trim(Json)};
                {error, Reason} -> {error, Reason}
            end;
        _ ->
            {error, <<"valid workflow run UUID is required">>}
    end.

%% ─── Generic guarded commit (run UPDATE + optional step-run INSERT) ──────────

commit(RunId0, SetList, StepRow) ->
    RunId = to_binary(RunId0),
    {SetSql, SetVars} = build_set(SetList),
    {StepCte, StepVars} = build_step_cte(StepRow),
    Sql = [
        StepCte,
        "update workflow_runs set ", SetSql, ", lease_until = null, updated_at = now() ",
        "where id = :'rid'::uuid and status in (", ?LIVE_STATUSES, ") ",
        "returning ", run_json("workflow_runs")
    ],
    Vars = [{"rid", RunId}] ++ SetVars ++ StepVars,
    case run_psql(Vars, Sql) of
        {ok, <<>>} -> {conflict, RunId};
        {ok, Json} -> {ok, string:trim(Json)};
        {error, Reason} -> {error, Reason}
    end.

build_set(SetList) ->
    Clauses = [Clause || {Clause, _} <- SetList],
    Vars = lists:append([V || {_, V} <- SetList]),
    {lists:join(", ", Clauses), Vars}.

build_step_cte(undefined) ->
    {"", []};
build_step_cte(Step) when is_map(Step) ->
    Cte = [
        "with ins as (insert into workflow_step_runs ",
        "(run_id, step_index, step_name, step_type, function_ref, attempt, status, ",
        "input, output, error, duration_ms, finished_at) values (",
        ":'rid'::uuid, :s_idx, :'s_name', :'s_type', :'s_fref', :s_attempt, :'s_status', ",
        ":'s_input'::jsonb, :'s_output'::jsonb, ", step_error_expr(Step), ", ",
        step_duration_expr(Step), ", now()) returning 1) "
    ],
    Vars = [
        {"s_idx", integer_to_binary(map_get_int(Step, index, 0))},
        {"s_name", clamp_text(map_get_bin(Step, name, <<"">>), ?MAX_NAME_LEN)},
        {"s_type", clamp_text(map_get_bin(Step, type, <<"activity">>), ?MAX_TYPE_LEN)},
        {"s_fref", clamp_text(map_get_bin(Step, function_ref, <<"">>), ?MAX_REF_LEN)},
        {"s_attempt", integer_to_binary(map_get_int(Step, attempt, 0))},
        {"s_status", map_get_bin(Step, status, <<"succeeded">>)},
        {"s_input", normalize_json(map_get_bin(Step, input, <<"null">>), <<"null">>)},
        {"s_output", normalize_json(map_get_bin(Step, output, <<"null">>), <<"null">>)}
    ] ++ step_error_var(Step) ++ step_duration_var(Step),
    {Cte, Vars}.

step_error_expr(Step) ->
    case maps:get(error, Step, undefined) of
        undefined -> "null";
        _ -> ":'s_error'"
    end.

step_error_var(Step) ->
    case maps:get(error, Step, undefined) of
        undefined -> [];
        Err -> [{"s_error", clamp_text(to_binary(Err), ?MAX_ERROR_LEN)}]
    end.

step_duration_expr(Step) ->
    case maps:get(duration_ms, Step, undefined) of
        undefined -> "null";
        _ -> ":s_dur"
    end.

step_duration_var(Step) ->
    case maps:get(duration_ms, Step, undefined) of
        undefined -> [];
        Dur when is_integer(Dur) -> [{"s_dur", integer_to_binary(Dur)}];
        _ -> []
    end.

%% ─── SET-clause builders (col fragment + bound vars) ─────────────────────────

set_int(Col, Value) when is_integer(Value) ->
    Var = Col,
    {Col ++ " = :" ++ Var, [{Var, integer_to_binary(Value)}]}.

set_text(Col, Value) ->
    Bin = clamp_text(to_binary(Value), ?MAX_ERROR_LEN),
    case Bin of
        <<>> -> {Col ++ " = null", []};
        _ -> {Col ++ " = :'" ++ Col ++ "'", [{Col, Bin}]}
    end.

set_json(Col, Json) ->
    Bin = normalize_json(to_binary(Json), <<"null">>),
    {Col ++ " = :'" ++ Col ++ "'::jsonb", [{Col, Bin}]}.

set_interval(Col, Ms) when is_integer(Ms) ->
    Var = Col ++ "_ms",
    {Col ++ " = now() + (:" ++ Var ++ " || ' milliseconds')::interval",
        [{Var, integer_to_binary(max(0, Ms))}]}.

%% ─── JSON projection fragments ───────────────────────────────────────────────

run_json(Alias) ->
    [run_json_object(Alias), "::text"].

run_json_object(Alias) ->
    [
        "jsonb_build_object(",
        "'id', ", Alias, ".id,",
        "'definitionId', ", Alias, ".definition_id,",
        "'definitionSlug', ", Alias, ".definition_slug,",
        "'status', ", Alias, ".status,",
        "'currentStepIndex', ", Alias, ".current_step_index,",
        "'attempt', ", Alias, ".attempt,",
        "'input', ", Alias, ".input,",
        "'context', ", Alias, ".context,",
        "'output', ", Alias, ".output,",
        "'lastError', ", Alias, ".last_error,",
        "'signals', ", Alias, ".signals,",
        "'idempotencyKey', ", Alias, ".idempotency_key,",
        "'wakeAt', ", Alias, ".wake_at,",
        "'createdAt', ", Alias, ".created_at,",
        "'updatedAt', ", Alias, ".updated_at,",
        "'startedAt', ", Alias, ".started_at,",
        "'finishedAt', ", Alias, ".finished_at",
        ")"
    ].

%% ─── psql plumbing ───────────────────────────────────────────────────────────

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
                        DatabaseUrl,
                        "-X", "-q", "-At",
                        "-v", "ON_ERROR_STOP=1",
                        "-c", binary_to_list(iolist_to_binary(Sql))
                    ],
                    Port = open_port({spawn_executable, Psql}, [
                        binary, exit_status, stderr_to_stdout, use_stdio,
                        {args, Args}
                    ]),
                    collect_port(Port, [], 0)
            end
    end.

var_arg(Name, Value) ->
    binary_to_list(iolist_to_binary([to_binary(Name), "=", to_binary(Value)])).

collect_port(Port, Chunks, Size) ->
    receive
        {Port, {data, Data}} ->
            NewSize = Size + byte_size(Data),
            case NewSize > ?MAX_OUTPUT_BYTES of
                true ->
                    close_port(Port),
                    {error, <<"workflow query exceeded byte limit">>};
                false ->
                    collect_port(Port, [Data | Chunks], NewSize)
            end;
        {Port, {exit_status, 0}} ->
            {ok, iolist_to_binary(lists:reverse(Chunks))};
        {Port, {exit_status, Status}} ->
            Output = iolist_to_binary(lists:reverse(Chunks)),
            {error, iolist_to_binary(io_lib:format("psql exited with status ~p: ~s", [Status, Output]))}
    after ?PSQL_TIMEOUT_MS ->
        close_port(Port),
        {error, <<"workflow query timed out">>}
    end.

close_port(Port) ->
    try erlang:port_close(Port) catch _:_ -> ok end.

database_url() ->
    case getenv(<<"LAMBDA_DATABASE_URL">>) of
        <<>> -> {error, <<"LAMBDA_DATABASE_URL is required">>};
        Value -> {ok, binary_to_list(Value)}
    end.

getenv(Name) ->
    case os:getenv(binary_to_list(Name)) of
        false -> <<>>;
        "" -> <<>>;
        Value -> list_to_binary(Value)
    end.

%% ─── helpers ─────────────────────────────────────────────────────────────────

identifier_kind(Identifier) ->
    case re:run(Identifier, "^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$", [{capture, none}]) of
        match ->
            uuid;
        nomatch ->
            case re:run(Identifier, "^[a-z0-9][a-z0-9-]{1,118}[a-z0-9]$", [{capture, none}]) of
                match -> slug;
                nomatch -> invalid
            end
    end.

normalize_json(<<>>, Default) -> Default;
normalize_json(Bin, Default) ->
    case string:trim(Bin) of
        <<>> -> Default;
        Trimmed -> Trimmed
    end.

%% Truncate a binary to at most Max bytes, on a UTF-8 boundary, so a value can
%% never violate a varchar(N)/octet_length CHECK constraint and fail-loop a run.
clamp_text(Bin, Max) when is_binary(Bin) ->
    case byte_size(Bin) =< Max of
        true -> Bin;
        false -> utf8_prefix(Bin, Max)
    end;
clamp_text(Other, Max) ->
    clamp_text(to_binary(Other), Max).

utf8_prefix(Bin, Max) ->
    Slice = binary:part(Bin, 0, Max),
    case unicode:characters_to_binary(Slice, utf8, utf8) of
        Valid when is_binary(Valid) -> Valid;
        _ -> trim_trailing_partial(Slice)
    end.

%% Drop up to 3 trailing bytes to land on a valid UTF-8 boundary.
trim_trailing_partial(Slice) ->
    trim_trailing_partial(Slice, 3).

trim_trailing_partial(Slice, 0) ->
    Slice;
trim_trailing_partial(Slice, N) ->
    Size = byte_size(Slice),
    case Size of
        0 -> <<>>;
        _ ->
            Shorter = binary:part(Slice, 0, Size - 1),
            case unicode:characters_to_binary(Shorter, utf8, utf8) of
                Valid when is_binary(Valid) -> Valid;
                _ -> trim_trailing_partial(Shorter, N - 1)
            end
    end.

clamp_int(Value, Min, Max, _Default) when is_integer(Value), Value >= Min, Value =< Max ->
    Value;
clamp_int(Value, Min, _Max, _Default) when is_integer(Value), Value < Min ->
    Min;
clamp_int(Value, _Min, Max, _Default) when is_integer(Value), Value > Max ->
    Max;
clamp_int(_Value, _Min, _Max, Default) ->
    Default.

map_get_int(Map, Key, Default) ->
    case maps:get(Key, Map, Default) of
        V when is_integer(V) -> V;
        _ -> Default
    end.

map_get_bin(Map, Key, Default) ->
    case maps:get(Key, Map, Default) of
        undefined -> Default;
        V -> to_binary(V)
    end.

to_binary(Value) when is_binary(Value) -> Value;
to_binary(Value) when is_list(Value) -> iolist_to_binary(Value);
to_binary(Value) when is_atom(Value) -> atom_to_binary(Value, utf8);
to_binary(Value) when is_integer(Value) -> integer_to_binary(Value);
to_binary(_) -> <<>>.
