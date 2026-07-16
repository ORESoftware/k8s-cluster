%% Workflow execution engine: a lightweight, Temporal-style durable job runner.
%%
%% Model: a workflow definition is a declarative ordered list of steps. Each step
%% is an `activity` (invoke a stored lambda function), a `sleep` (durable timer),
%% or a `waitSignal` (block on an external signal). Run state is a durable
%% step-state machine persisted in Postgres after every step (workflow_store);
%% the engine holds no authoritative in-memory state, so a crash/restart resumes
%% automatically — leased-but-orphaned runs are reclaimed once their lease lapses.
%%
%% This singleton gen_server polls Postgres for due runs (atomic lease claim with
%% FOR UPDATE SKIP LOCKED, safe across replicas), then dispatches each due run to
%% a short-lived worker process that advances it exactly one step and persists the
%% transition. Activities are at-least-once (lease expiry => retry), so activity
%% lambdas should be idempotent — the same contract Temporal places on activities.
-module(workflow_engine).
-behaviour(gen_server).

-export([
    start/0,
    start_link/0,
    start_for_gleam/0,
    child_spec/0,
    enabled/0,
    start_run/3,
    start_run_from_body/1,
    signal_run/3,
    signal_from_body/2,
    cancel_run/1,
    get_run/1,
    list_runs/2,
    metrics/0,
    nudge/0,
    %% Exported for unit tests (pure helpers).
    backoff_ms/2,
    max_attempts/1
]).

-export([init/1, handle_call/3, handle_cast/2, handle_info/2, terminate/2]).

-define(SERVER, ?MODULE).
-define(METRICS, workflow_engine_metrics).
-define(DEFAULT_POLL_MS, 1000).
-define(DEFAULT_MAX_INFLIGHT, 16).
-define(DEFAULT_LEASE_MS, 60000).
-define(DEFAULT_CLAIM_BATCH, 25).
-define(DEFAULT_ACTIVITY_TIMEOUT_MS, 30000).
-define(DEFAULT_ACTIVITY_IDLE_MS, 300000).
-define(DEFAULT_MAX_ATTEMPTS, 3).
-define(MAX_ATTEMPTS_CAP, 1000).
-define(DEFAULT_BACKOFF_MS, 1000).
-define(DEFAULT_BACKOFF_FACTOR, 2.0).
-define(DEFAULT_MAX_BACKOFF_MS, 60000).
%% Hard ceiling on accumulated run context (sum of step outputs). Exceeding this
%% terminally fails the run instead of looping on un-persistable oversized writes.
-define(MAX_CONTEXT_BYTES, 4194304).

%% ─── Public API (delegates to the store; nudges the scheduler) ───────────────

enabled() ->
    workflow_store:available() andalso env_flag(<<"WORKFLOW_ENGINE_ENABLED">>, true).

%% Parse a start request body ({definitionId|definitionSlug, input?, idempotencyKey?})
%% and create the run. Used by the HTTP and NATS surfaces.
start_run_from_body(Body) ->
    case decode_object(Body) of
        {ok, Obj} ->
            case first_present(Obj, [<<"definitionId">>, <<"definitionSlug">>, <<"definition">>, <<"slug">>]) of
                <<>> ->
                    {error, <<"definitionId or definitionSlug is required">>};
                DefRef ->
                    Input = encode(maps:get(<<"input">>, Obj, null)),
                    Idem = first_present(Obj, [<<"idempotencyKey">>, <<"idempotency_key">>]),
                    start_run(DefRef, Input, Idem)
            end;
        {error, Reason} ->
            {error, Reason}
    end.

start_run(DefRef, InputJson, IdempotencyKey) ->
    case workflow_store:create_run(DefRef, InputJson, IdempotencyKey) of
        {ok, RunJson} ->
            bump(runs_started, 1),
            nudge(),
            publish_event(#{<<"event">> => <<"run.started">>, <<"run">> => raw(RunJson)}),
            {ok, RunJson};
        Error ->
            Error
    end.

%% Parse a signal request body ({name, payload?}) and deliver it to a run.
signal_from_body(RunId, Body) ->
    case decode_object(Body) of
        {ok, Obj} ->
            case maps:get(<<"name">>, Obj, undefined) of
                undefined ->
                    {error, <<"signal name is required">>};
                Name ->
                    NameJson = encode(Name),
                    PayloadJson = encode(maps:get(<<"payload">>, Obj, null)),
                    signal_run(RunId, NameJson, PayloadJson)
            end;
        {error, Reason} ->
            {error, Reason}
    end.

signal_run(RunId, NameJson, PayloadJson) ->
    case workflow_store:deliver_signal(RunId, NameJson, PayloadJson) of
        {ok, RunJson} ->
            bump(signals_delivered, 1),
            nudge(),
            {ok, RunJson};
        Error ->
            Error
    end.

cancel_run(RunId) ->
    case workflow_store:cancel_run(RunId) of
        {ok, RunJson} ->
            bump(runs_canceled, 1),
            publish_event(#{<<"event">> => <<"run.canceled">>, <<"runId">> => to_binary(RunId)}),
            {ok, RunJson};
        {conflict, _} ->
            {error, <<"workflow run is not cancelable">>};
        Error ->
            Error
    end.

get_run(RunId) -> workflow_store:get_run_with_steps(RunId).

list_runs(DefRef, Limit) -> workflow_store:list_runs(DefRef, Limit).

%% Ask the scheduler to claim immediately rather than waiting for the next tick.
nudge() ->
    case whereis(?SERVER) of
        undefined -> ok;
        _ -> gen_server:cast(?SERVER, wake)
    end.

%% ─── Supervision ─────────────────────────────────────────────────────────────

child_spec() ->
    #{
        id => ?MODULE,
        start => {?MODULE, start_link, []},
        restart => permanent,
        shutdown => 5000,
        type => worker,
        modules => [?MODULE]
    }.

start_link() ->
    gen_server:start_link({local, ?SERVER}, ?MODULE, [], []).

%% Detached singleton start (not linked to the caller), mirroring lambda_nats:
%% an engine fault must not take down the HTTP server. Durable state lives in
%% Postgres, so a fresh start re-claims orphaned runs once their leases lapse.
start() ->
    case whereis(?SERVER) of
        undefined -> gen_server:start({local, ?SERVER}, ?MODULE, [], []);
        Pid -> {ok, Pid}
    end.

%% Gleam FFI entrypoint: start the engine and return the `nil` Gleam expects.
start_for_gleam() ->
    _ = start(),
    nil.

%% ─── gen_server ──────────────────────────────────────────────────────────────

init([]) ->
    ensure_metrics(),
    Enabled = enabled(),
    State = #{
        enabled => Enabled,
        poll_ms => env_int(<<"WORKFLOW_POLL_MS">>, ?DEFAULT_POLL_MS, 50, 600000),
        max_inflight => env_int(<<"WORKFLOW_MAX_INFLIGHT">>, ?DEFAULT_MAX_INFLIGHT, 1, 512),
        lease_ms => env_int(<<"WORKFLOW_LEASE_MS">>, ?DEFAULT_LEASE_MS, 1000, 600000),
        batch => env_int(<<"WORKFLOW_CLAIM_BATCH">>, ?DEFAULT_CLAIM_BATCH, 1, 200),
        inflight => #{}
    },
    case Enabled of
        true ->
            io:format("workflow-engine enabled; polling every ~p ms~n", [maps:get(poll_ms, State)]),
            self() ! tick;
        false ->
            io:format("workflow-engine disabled (no LAMBDA_DATABASE_URL or WORKFLOW_ENGINE_ENABLED=0)~n", [])
    end,
    {ok, State}.

handle_call(_Request, _From, State) ->
    {reply, {error, unsupported}, State}.

handle_cast(wake, State = #{enabled := true}) ->
    {noreply, claim_and_dispatch(State)};
handle_cast(_Msg, State) ->
    {noreply, State}.

handle_info(tick, State = #{enabled := true, poll_ms := PollMs}) ->
    State1 = claim_and_dispatch(State),
    erlang:send_after(PollMs, self(), tick),
    {noreply, State1};
handle_info(tick, State) ->
    {noreply, State};
handle_info({'DOWN', _Ref, process, Pid, Reason}, State = #{inflight := Inflight}) ->
    case maps:take(Pid, Inflight) of
        {RunId, Rest} ->
            case Reason of
                normal -> ok;
                _ ->
                    bump(worker_crashes, 1),
                    io:format("workflow-engine worker for run ~s crashed: ~p~n", [RunId, Reason])
            end,
            {noreply, State#{inflight => Rest}};
        error ->
            {noreply, State}
    end;
handle_info(_Msg, State) ->
    {noreply, State}.

terminate(_Reason, _State) ->
    ok.

%% ─── Scheduling ──────────────────────────────────────────────────────────────

%% Must never raise: the engine is a detached singleton (no supervisor), and
%% handle_info(tick) reschedules itself, so an exception escaping here would stop
%% the scheduler permanently. Any fault is logged and the state left untouched.
claim_and_dispatch(State) ->
    try
        do_claim_and_dispatch(State)
    catch
        Class:Reason:Stacktrace ->
            bump(claim_errors, 1),
            io:format("workflow-engine claim loop fault: ~p:~p~n~p~n", [Class, Reason, Stacktrace]),
            State
    end.

do_claim_and_dispatch(State = #{inflight := Inflight, max_inflight := Max, batch := Batch, lease_ms := LeaseMs}) ->
    Free = Max - maps:size(Inflight),
    case Free =< 0 of
        true ->
            State;
        false ->
            bump(claims_total, 1),
            case workflow_store:claim_due(min(Free, Batch), LeaseMs) of
                {ok, []} ->
                    State;
                {ok, Runs} ->
                    lists:foldl(fun dispatch_run/2, State, Runs);
                {error, Reason} ->
                    bump(claim_errors, 1),
                    io:format("workflow-engine claim error: ~s~n", [safe(Reason)]),
                    State
            end
    end.

dispatch_run(Run, State = #{inflight := Inflight}) when is_map(Run) ->
    {Pid, _MRef} = spawn_monitor(fun() -> process_run(Run) end),
    State#{inflight => Inflight#{Pid => map_bin(Run, <<"id">>, <<"?">>)}};
dispatch_run(_Run, State) ->
    State.

%% ─── Worker: advance one run by exactly one step ─────────────────────────────

%% A worker advances one run by one step. Any unexpected exception here (a
%% malformed definition, a non-encodable activity output, etc.) is caught and
%% routed through the retry policy instead of killing the worker silently —
%% otherwise the lease would expire and re-run the same poison step forever
%% without ever counting an attempt.
process_run(Run) ->
    RunId = map_bin(Run, <<"id">>, <<>>),
    try
        do_process_run(RunId, Run)
    catch
        Class:Reason:Stacktrace ->
            bump(worker_exceptions, 1),
            io:format("workflow-engine step crashed for run ~s: ~p:~p~n~p~n",
                [RunId, Class, Reason, Stacktrace]),
            record_crash(RunId, Run,
                iolist_to_binary(io_lib:format("workflow step crashed: ~p:~p", [Class, Reason])))
    end.

do_process_run(RunId, Run) ->
    Status = map_bin(Run, <<"status">>, <<"pending">>),
    Steps = map_get(Run, <<"steps">>, []),
    Idx = map_int(Run, <<"currentStepIndex">>, 0),
    Total = length(Steps),
    case Idx >= Total of
        true ->
            complete_run(RunId, Run);
        false ->
            Step = lists:nth(Idx + 1, Steps),
            dispatch_step(RunId, Run, Status, Step, Idx, Total)
    end.

%% Bounded failure for a caught exception: apply the current step's retry policy,
%% so transient faults retry and deterministic poison exhausts maxAttempts and
%% terminally fails. Uses best-effort commits and only total-safe map access.
record_crash(RunId, Run, ErrBin) ->
    Steps = map_get(Run, <<"steps">>, []),
    Idx = map_int(Run, <<"currentStepIndex">>, 0),
    Step =
        case is_list(Steps) andalso Idx >= 0 andalso Idx < length(Steps) of
            true -> lists:nth(Idx + 1, Steps);
            false -> #{}
        end,
    Attempt = map_int(Run, <<"attempt">>, 0),
    NewAttempt = Attempt + 1,
    Retry = retry_config(Run, Step),
    MaxAttempts = max_attempts(Retry),
    StepName = step_name(Step, Idx),
    StepRow = step_row(Idx, StepName, step_type(Step), <<>>, Attempt,
        <<"failed">>, undefined, undefined, ErrBin, undefined),
    case NewAttempt < MaxAttempts of
        true ->
            ignore_commit(workflow_store:fail_retry(RunId, StepRow, NewAttempt, backoff_ms(Retry, Attempt), ErrBin));
        false ->
            ignore_commit(workflow_store:fail_terminal(RunId, StepRow, ErrBin))
    end.

complete_run(RunId, Run) ->
    Context = map_get(Run, <<"context">>, #{}),
    ContextJson = encode(Context),
    handle_commit(
        workflow_store:succeed_complete(RunId, undefined, ContextJson, ContextJson),
        RunId,
        fun(RunJson) ->
            bump(runs_completed, 1),
            publish_event(#{<<"event">> => <<"run.completed">>, <<"run">> => raw(RunJson)})
        end
    ).

dispatch_step(RunId, Run, Status, Step, Idx, Total) ->
    case step_type(Step) of
        <<"activity">> -> run_activity(RunId, Run, Step, Idx, Total);
        <<"sleep">> -> run_sleep(RunId, Step, Idx);
        <<"waitSignal">> -> run_wait_signal(RunId, Run, Status, Step, Idx, Total);
        Other ->
            terminal_failure(RunId, Step, Idx,
                iolist_to_binary([<<"unknown workflow step type: ">>, Other]))
    end.

%% ── activity ──

run_activity(RunId, Run, Step, Idx, Total) ->
    case activity_function_ref(Step) of
        {error, Reason} ->
            terminal_failure(RunId, Step, Idx, Reason);
        {ok, FunctionRef} ->
            Context = map_get(Run, <<"context">>, #{}),
            RunInput = map_get(Run, <<"input">>, null),
            StepInput = map_get(Step, <<"input">>, #{}),
            Payload = encode(#{
                <<"runId">> => RunId,
                <<"step">> => step_name(Step, Idx),
                <<"input">> => StepInput,
                <<"context">> => Context,
                <<"runInput">> => RunInput
            }),
            TimeoutMs = map_int(Step, <<"timeoutMs">>, ?DEFAULT_ACTIVITY_TIMEOUT_MS),
            Started = erlang:monotonic_time(millisecond),
            Result = lambda_child_runner:invoke(
                activity_command(), FunctionRef, Payload, ?DEFAULT_ACTIVITY_IDLE_MS, TimeoutMs),
            DurationMs = erlang:monotonic_time(millisecond) - Started,
            case Result of
                {ok, Output} ->
                    activity_success(RunId, Run, Step, Idx, Total, FunctionRef, StepInput, Output, DurationMs);
                {error, Error} ->
                    activity_failure(RunId, Run, Step, Idx, FunctionRef, StepInput, Error, DurationMs)
            end
    end.

activity_success(RunId, Run, Step, Idx, Total, FunctionRef, StepInput, Output, DurationMs) ->
    StepName = step_name(Step, Idx),
    OutputValue = decode_output(Output),
    Context = map_get(Run, <<"context">>, #{}),
    NewContext = Context#{StepName => OutputValue},
    NewContextJson = encode(NewContext),
    case byte_size(NewContextJson) > ?MAX_CONTEXT_BYTES of
        true ->
            terminal_failure(RunId, Step, Idx,
                iolist_to_binary([<<"workflow context exceeded ">>,
                    integer_to_binary(?MAX_CONTEXT_BYTES), <<" bytes">>]));
        false ->
            activity_success_commit(RunId, Run, Idx, Total, FunctionRef,
                StepName, StepInput, OutputValue, NewContextJson, DurationMs)
    end.

activity_success_commit(RunId, Run, Idx, Total, FunctionRef, StepName, StepInput, OutputValue, NewContextJson, DurationMs) ->
    OutputJson = encode(OutputValue),
    StepRow = step_row(Idx, StepName, <<"activity">>, FunctionRef, map_int(Run, <<"attempt">>, 0),
        <<"succeeded">>, encode(StepInput), OutputJson, undefined, DurationMs),
    case Idx + 1 >= Total of
        true ->
            handle_commit(
                workflow_store:succeed_complete(RunId, StepRow, OutputJson, NewContextJson),
                RunId,
                fun(RunJson) ->
                    bump(steps_succeeded, 1),
                    bump(runs_completed, 1),
                    publish_event(#{<<"event">> => <<"step.succeeded">>, <<"runId">> => RunId, <<"step">> => StepName}),
                    publish_event(#{<<"event">> => <<"run.completed">>, <<"run">> => raw(RunJson)})
                end);
        false ->
            handle_commit(
                workflow_store:succeed_advance(RunId, StepRow, Idx + 1, NewContextJson),
                RunId,
                fun(_RunJson) ->
                    bump(steps_succeeded, 1),
                    publish_event(#{<<"event">> => <<"step.succeeded">>, <<"runId">> => RunId, <<"step">> => StepName}),
                    nudge()
                end)
    end.

activity_failure(RunId, Run, Step, Idx, FunctionRef, StepInput, Error, DurationMs) ->
    StepName = step_name(Step, Idx),
    Attempt = map_int(Run, <<"attempt">>, 0),
    NewAttempt = Attempt + 1,
    Retry = retry_config(Run, Step),
    MaxAttempts = max_attempts(Retry),
    ErrBin = safe(Error),
    StepRow = step_row(Idx, StepName, <<"activity">>, FunctionRef, Attempt,
        <<"failed">>, encode(StepInput), undefined, ErrBin, DurationMs),
    case NewAttempt < MaxAttempts of
        true ->
            Backoff = backoff_ms(Retry, Attempt),
            handle_commit(
                workflow_store:fail_retry(RunId, StepRow, NewAttempt, Backoff, ErrBin),
                RunId,
                fun(_RunJson) ->
                    bump(steps_retried, 1),
                    publish_event(#{<<"event">> => <<"step.retry">>, <<"runId">> => RunId,
                        <<"step">> => StepName, <<"attempt">> => NewAttempt, <<"backoffMs">> => Backoff})
                end);
        false ->
            terminal_failure_row(RunId, StepRow, StepName, ErrBin)
    end.

%% ── sleep ──

run_sleep(RunId, Step, Idx) ->
    DurationMs = map_int(Step, <<"durationMs">>, 0),
    StepName = step_name(Step, Idx),
    StepRow = step_row(Idx, StepName, <<"sleep">>, <<>>, 0, <<"succeeded">>,
        encode(#{<<"durationMs">> => DurationMs}), undefined, undefined, 0),
    handle_commit(
        workflow_store:enter_sleep(RunId, StepRow, DurationMs, Idx + 1),
        RunId,
        fun(_RunJson) ->
            bump(timers_started, 1),
            publish_event(#{<<"event">> => <<"step.sleep">>, <<"runId">> => RunId,
                <<"step">> => StepName, <<"durationMs">> => DurationMs})
        end).

%% ── waitSignal ──

run_wait_signal(RunId, Run, Status, Step, Idx, _Total) ->
    SignalName = map_bin(Step, <<"signalName">>, <<>>),
    Signals = map_get(Run, <<"signals">>, []),
    StepName = step_name(Step, Idx),
    case find_signal(Signals, SignalName, 0) of
        {ok, Position, SignalPayload} ->
            Context = map_get(Run, <<"context">>, #{}),
            NewContext = Context#{StepName => SignalPayload},
            NewContextJson = encode(NewContext),
            StepRow = step_row(Idx, StepName, <<"waitSignal">>, <<>>, 0, <<"succeeded">>,
                encode(#{<<"signalName">> => SignalName}), encode(SignalPayload), undefined, 0),
            handle_commit(
                workflow_store:consume_signal(RunId, StepRow, Idx + 1, NewContextJson, Position),
                RunId,
                fun(_RunJson) ->
                    bump(signals_consumed, 1),
                    publish_event(#{<<"event">> => <<"step.signal">>, <<"runId">> => RunId,
                        <<"step">> => StepName, <<"signal">> => SignalName}),
                    nudge()
                end);
        not_found ->
            wait_no_signal(RunId, Run, Status, Step, Idx, StepName)
    end.

wait_no_signal(RunId, Run, <<"waiting">>, Step, Idx, _StepName) ->
    %% Resumed while waiting: either the deadline fired (timeout) or a
    %% non-matching signal woke us early (re-park, keeping the deadline).
    NowMs = map_int(Run, <<"nowMs">>, 0),
    case map_get(Run, <<"waitDeadlineMs">>, null) of
        null ->
            ignore_commit(workflow_store:repark_wait(RunId));
        Deadline when is_number(Deadline), NowMs >= Deadline ->
            terminal_failure(RunId, Step, Idx,
                iolist_to_binary([<<"signal wait timeout: ">>, map_bin(Step, <<"signalName">>, <<"?">>)]));
        _ ->
            ignore_commit(workflow_store:repark_wait(RunId))
    end;
wait_no_signal(RunId, _Run, _Status, Step, _Idx, _StepName) ->
    %% First entry into the wait: park until a signal or the timeout.
    Deadline =
        case map_int(Step, <<"waitTimeoutMs">>, 0) of
            0 -> infinity;
            Ms -> Ms
        end,
    ignore_commit(workflow_store:park_wait(RunId, Deadline)),
    bump(waits_started, 1).

%% ── failure helpers ──

terminal_failure(RunId, Step, Idx, ErrBin) ->
    StepName = step_name(Step, Idx),
    StepRow = step_row(Idx, StepName, step_type(Step), <<>>, 0, <<"failed">>,
        undefined, undefined, ErrBin, undefined),
    terminal_failure_row(RunId, StepRow, StepName, ErrBin).

terminal_failure_row(RunId, StepRow, StepName, ErrBin) ->
    handle_commit(
        workflow_store:fail_terminal(RunId, StepRow, ErrBin),
        RunId,
        fun(RunJson) ->
            bump(steps_failed, 1),
            bump(runs_failed, 1),
            publish_event(#{<<"event">> => <<"step.failed">>, <<"runId">> => RunId,
                <<"step">> => StepName, <<"error">> => ErrBin}),
            publish_event(#{<<"event">> => <<"run.failed">>, <<"run">> => raw(RunJson)})
        end).

%% Apply OnOk on a successful commit; treat a guard conflict (run concurrently
%% cancelled/changed) as a benign no-op; log hard errors.
handle_commit({ok, RunJson}, _RunId, OnOk) ->
    OnOk(RunJson),
    ok;
handle_commit({conflict, RunId}, _RunId2, _OnOk) ->
    bump(commit_conflicts, 1),
    io:format("workflow-engine commit skipped (run ~s changed concurrently)~n", [RunId]),
    ok;
handle_commit({error, Reason}, RunId, _OnOk) ->
    bump(commit_errors, 1),
    io:format("workflow-engine commit error for run ~s: ~s~n", [RunId, safe(Reason)]),
    ok.

ignore_commit({ok, _}) -> ok;
ignore_commit({conflict, _}) -> ok;
ignore_commit({error, Reason}) ->
    bump(commit_errors, 1),
    io:format("workflow-engine park error: ~s~n", [safe(Reason)]),
    ok.

%% ─── Step/run helpers ────────────────────────────────────────────────────────

step_type(Step) when is_map(Step) ->
    case map_bin(Step, <<"type">>, <<"activity">>) of
        <<>> -> <<"activity">>;
        T -> T
    end;
step_type(_) -> <<"activity">>.

step_name(Step, Idx) ->
    case map_bin(Step, <<"name">>, <<>>) of
        <<>> -> iolist_to_binary([<<"step-">>, integer_to_binary(Idx)]);
        Name -> Name
    end.

activity_function_ref(Step) ->
    case map_bin(Step, <<"functionId">>, <<>>) of
        <<>> ->
            case map_bin(Step, <<"functionSlug">>, <<>>) of
                <<>> -> {error, <<"activity step requires functionId or functionSlug">>};
                Slug -> {ok, Slug}
            end;
        Id ->
            {ok, Id}
    end.

step_row(Idx, Name, Type, FunctionRef, Attempt, Status, Input, Output, Error, DurationMs) ->
    Base = #{
        index => Idx,
        name => Name,
        type => Type,
        function_ref => FunctionRef,
        attempt => Attempt,
        status => Status
    },
    Base1 = maybe_put(input, Input, Base),
    Base2 = maybe_put(output, Output, Base1),
    Base3 = maybe_put(error, Error, Base2),
    maybe_put(duration_ms, DurationMs, Base3).

maybe_put(_Key, undefined, Map) -> Map;
maybe_put(Key, Value, Map) -> Map#{Key => Value}.

retry_config(Run, Step) ->
    DefaultRetry = map_get(Run, <<"defaultRetry">>, #{}),
    StepRetry = map_get(Step, <<"retry">>, #{}),
    case {is_map(DefaultRetry), is_map(StepRetry)} of
        {true, true} -> maps:merge(DefaultRetry, StepRetry);
        {true, false} -> DefaultRetry;
        {false, true} -> StepRetry;
        _ -> #{}
    end.

%% Cap maxAttempts to a sane range so a definition can't request effectively
%% unbounded retries (which would also blow up the backoff exponent).
max_attempts(Retry) ->
    case map_num(Retry, <<"maxAttempts">>, ?DEFAULT_MAX_ATTEMPTS) of
        N when N < 1 -> 1;
        N when N > ?MAX_ATTEMPTS_CAP -> ?MAX_ATTEMPTS_CAP;
        N -> N
    end.

backoff_ms(Retry, Attempt) ->
    Base = max(0, map_num(Retry, <<"backoffMs">>, ?DEFAULT_BACKOFF_MS)),
    Factor = max(1.0, map_float(Retry, <<"backoffFactor">>, ?DEFAULT_BACKOFF_FACTOR)),
    MaxBackoff = map_num(Retry, <<"maxBackoffMs">>, ?DEFAULT_MAX_BACKOFF_MS),
    %% Clamp the exponent: math:pow overflows to a float exception for large
    %% attempt counts, and the result is capped at MaxBackoff anyway.
    Exp = min(Attempt, 64),
    Scaled = Base * math:pow(Factor, Exp),
    min(MaxBackoff, round(min(Scaled, 1.0e15))).

find_signal([], _Name, _Pos) ->
    not_found;
find_signal([Signal | Rest], Name, Pos) when is_map(Signal) ->
    case map_bin(Signal, <<"name">>, <<>>) =:= Name of
        true -> {ok, Pos, map_get(Signal, <<"payload">>, null)};
        false -> find_signal(Rest, Name, Pos + 1)
    end;
find_signal([_ | Rest], Name, Pos) ->
    find_signal(Rest, Name, Pos + 1).

decode_output(Output) when is_binary(Output) ->
    case string:trim(Output) of
        <<>> -> null;
        Trimmed ->
            try json:decode(Trimmed) of
                Decoded -> Decoded
            catch
                _:_ -> Output
            end
    end;
decode_output(Output) -> Output.

activity_command() ->
    case getenv(<<"LAMBDA_NODEJS_HOST_COMMAND">>) of
        <<>> ->
            "env -i PATH=\"$PATH\" NODE_ENV=production NODE_NO_WARNINGS=1 "
            "node --permission --allow-net --allow-fs-read=child-runtimes "
            "--allow-fs-read=../../../libs/nats/subject-defs/generated/javascript "
            "child-runtimes/js-function-runner.mjs";
        Value ->
            binary_to_list(Value)
    end.

%% ─── NATS events (best-effort fan-out) ───────────────────────────────────────

publish_event(Event) ->
    Subject = event_subject(),
    try
        Enriched = Event#{<<"ts">> => iso_now()},
        _ = lambda_nats:publish(Subject, encode(Enriched)),
        ok
    catch
        _:_ -> ok
    end.

event_subject() ->
    case getenv(<<"NATS_WORKFLOW_EVENT_SUBJECT">>) of
        <<>> -> <<"dd.remote.workflows.events">>;
        Value -> Value
    end.

%% raw/1 embeds an already-encoded JSON string as a structured value in an event
%% by decoding it; on failure it degrades to the raw string.
raw(Json) ->
    try json:decode(iolist_to_binary(Json)) of
        Decoded -> Decoded
    catch
        _:_ -> iolist_to_binary(Json)
    end.

iso_now() ->
    list_to_binary(calendar:system_time_to_rfc3339(erlang:system_time(second), [{offset, "Z"}])).

%% ─── Metrics ─────────────────────────────────────────────────────────────────

ensure_metrics() ->
    case ets:info(?METRICS) of
        undefined ->
            try ets:new(?METRICS, [named_table, public, set, {write_concurrency, true}])
            catch error:badarg -> ?METRICS end;
        _ ->
            ?METRICS
    end.

bump(Key, By) ->
    try ets:update_counter(?METRICS, Key, By, {Key, 0})
    catch _:_ -> 0 end.

metric(Key) ->
    case catch ets:lookup(?METRICS, Key) of
        [{Key, V}] -> V;
        _ -> 0
    end.

metrics() ->
    Counters = [
        {<<"workflow_runs_started_total">>, runs_started},
        {<<"workflow_runs_completed_total">>, runs_completed},
        {<<"workflow_runs_failed_total">>, runs_failed},
        {<<"workflow_runs_canceled_total">>, runs_canceled},
        {<<"workflow_steps_succeeded_total">>, steps_succeeded},
        {<<"workflow_steps_failed_total">>, steps_failed},
        {<<"workflow_steps_retried_total">>, steps_retried},
        {<<"workflow_timers_started_total">>, timers_started},
        {<<"workflow_waits_started_total">>, waits_started},
        {<<"workflow_signals_delivered_total">>, signals_delivered},
        {<<"workflow_signals_consumed_total">>, signals_consumed},
        {<<"workflow_claims_total">>, claims_total},
        {<<"workflow_claim_errors_total">>, claim_errors},
        {<<"workflow_commit_conflicts_total">>, commit_conflicts},
        {<<"workflow_commit_errors_total">>, commit_errors},
        {<<"workflow_worker_crashes_total">>, worker_crashes},
        {<<"workflow_worker_exceptions_total">>, worker_exceptions}
    ],
    Lines = [[Name, <<" ">>, integer_to_binary(metric(Key)), <<"\n">>] || {Name, Key} <- Counters],
    iolist_to_binary([
        <<"# HELP workflow_engine Workflow execution engine counters\n">>,
        <<"# TYPE workflow_runs_started_total counter\n">>,
        Lines
    ]).

%% ─── env + map helpers ───────────────────────────────────────────────────────

decode_object(Body) ->
    case string:trim(to_binary(Body)) of
        <<>> ->
            {error, <<"request body is required">>};
        Bin ->
            try json:decode(Bin) of
                Obj when is_map(Obj) -> {ok, Obj};
                _ -> {error, <<"request body must be a JSON object">>}
            catch
                _:_ -> {error, <<"invalid JSON body">>}
            end
    end.

first_present(_Obj, []) ->
    <<>>;
first_present(Obj, [Key | Rest]) ->
    case maps:get(Key, Obj, undefined) of
        V when is_binary(V), V =/= <<>> -> V;
        _ -> first_present(Obj, Rest)
    end.

getenv(Name) ->
    case os:getenv(binary_to_list(Name)) of
        false -> <<>>;
        "" -> <<>>;
        Value -> list_to_binary(Value)
    end.

env_int(Name, Default, Min, Max) ->
    case getenv(Name) of
        <<>> -> Default;
        Bin ->
            case catch binary_to_integer(Bin) of
                V when is_integer(V), V >= Min, V =< Max -> V;
                _ -> Default
            end
    end.

env_flag(Name, Default) ->
    case getenv(Name) of
        <<>> -> Default;
        <<"0">> -> false;
        <<"false">> -> false;
        <<"no">> -> false;
        _ -> true
    end.

encode(Term) ->
    iolist_to_binary(json:encode(Term)).

map_get(Map, Key, Default) when is_map(Map) -> maps:get(Key, Map, Default);
map_get(_Map, _Key, Default) -> Default.

map_bin(Map, Key, Default) ->
    case map_get(Map, Key, Default) of
        V when is_binary(V) -> V;
        V when is_list(V) -> iolist_to_binary(V);
        _ -> Default
    end.

map_int(Map, Key, Default) ->
    case map_get(Map, Key, Default) of
        V when is_integer(V) -> V;
        V when is_float(V) -> round(V);
        _ -> Default
    end.

map_num(Map, Key, Default) ->
    case map_get(Map, Key, Default) of
        V when is_integer(V) -> V;
        V when is_float(V) -> round(V);
        _ -> Default
    end.

map_float(Map, Key, Default) ->
    case map_get(Map, Key, Default) of
        V when is_number(V) -> V * 1.0;
        _ -> Default
    end.

safe(Bin) when is_binary(Bin) -> Bin;
safe(List) when is_list(List) -> iolist_to_binary(List);
safe(Other) -> iolist_to_binary(io_lib:format("~p", [Other])).

to_binary(V) when is_binary(V) -> V;
to_binary(V) when is_list(V) -> iolist_to_binary(V);
to_binary(V) when is_atom(V) -> atom_to_binary(V, utf8);
to_binary(V) when is_integer(V) -> integer_to_binary(V);
to_binary(_) -> <<>>.
