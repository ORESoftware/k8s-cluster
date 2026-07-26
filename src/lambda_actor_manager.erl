%% Local routing registry for dynamically supervised durable actor processes.
%% It never executes user code itself; the manager stays responsive while each
%% actor's gen_server mailbox serializes its own requests.
-module(lambda_actor_manager).
-behaviour(gen_server).

-export([
    start_link/0,
    invoke/6,
    invoke_alarm/6,
    reset/2,
    get_state/2,
    snapshot/0,
    metrics/0,
    enabled/0
]).
-export([init/1, handle_call/3, handle_cast/2, handle_info/2, terminate/2]).

-define(SERVER, ?MODULE).
-define(METRICS, lambda_actor_metrics).
-define(DEFAULT_MAX_ACTIVE, 10000).
-define(DEFAULT_MAX_QUEUE, 100).

start_link() ->
    gen_server:start_link({local, ?SERVER}, ?MODULE, [], []).

enabled() ->
    lambda_actor_store:available() andalso env_flag(<<"ACTOR_ENGINE_ENABLED">>, true).

invoke(Command, FunctionRef, ActorKey, Payload, ChildIdleMs, TimeoutMs) ->
    bump(invocations_total, 1),
    case actor_pid(FunctionRef, ActorKey) of
        {ok, Pid} ->
            CallTimeout = call_timeout(TimeoutMs),
            case queue_available(Pid) of
                true ->
                    safe_actor_call(fun() ->
                        lambda_actor:invoke(
                            Pid,
                            Command,
                            Payload,
                            ChildIdleMs,
                            TimeoutMs,
                            CallTimeout
                        )
                    end);
                false ->
                    bump(queue_rejections_total, 1),
                    {error, <<"actor queue depth limit reached">>}
            end;
        {error, Reason} ->
            bump(invocation_failures_total, 1),
            {error, Reason}
    end.

invoke_alarm(Command, FunctionRef, ActorKey, ScheduledAt, ChildIdleMs, TimeoutMs) ->
    bump(alarm_invocations_total, 1),
    case actor_pid(FunctionRef, ActorKey) of
        {ok, Pid} ->
            CallTimeout = call_timeout(TimeoutMs),
            case queue_available(Pid) of
                true ->
                    case safe_actor_call(fun() ->
                        lambda_actor:invoke_alarm(
                            Pid,
                            Command,
                            ScheduledAt,
                            ChildIdleMs,
                            TimeoutMs,
                            CallTimeout
                        )
                    end) of
                        {ok, _} = Success -> Success;
                        {error, Reason} ->
                            bump(alarm_failures_total, 1),
                            {error, Reason}
                    end;
                false ->
                    bump(queue_rejections_total, 1),
                    bump(alarm_failures_total, 1),
                    {error, <<"actor queue depth limit reached">>}
            end;
        {error, Reason} ->
            bump(alarm_failures_total, 1),
            {error, Reason}
    end.

reset(FunctionRef, ActorKey) ->
    case actor_pid(FunctionRef, ActorKey) of
        {ok, Pid} ->
            QueueWaitMs = env_int(<<"ACTOR_QUEUE_WAIT_MS">>, 30000, 0, 300000),
            LeaseMs = env_int(<<"ACTOR_LEASE_MS">>, 310000, 1000, 360000),
            safe_actor_call(fun() ->
                lambda_actor:reset(
                    Pid,
                    QueueWaitMs,
                    LeaseMs,
                    QueueWaitMs + LeaseMs + 10000
                )
            end);
        {error, Reason} ->
            {error, Reason}
    end.

get_state(FunctionRef, ActorKey) ->
    case lambda_actor_store:get_state(FunctionRef, ActorKey) of
        {ok, Actor} ->
            {ok, iolist_to_binary(json:encode(#{
                <<"ok">> => true,
                <<"actor">> => Actor
            }))};
        {error, Reason} ->
            {error, Reason}
    end.

snapshot() ->
    case whereis(?SERVER) of
        Pid when is_pid(Pid) ->
            try gen_server:call(Pid, snapshot, 2000) of
                Snapshot -> Snapshot
            catch
                _:_ -> <<"{\"ok\":false,\"activeActors\":0}">>
            end;
        _ ->
            <<"{\"ok\":false,\"activeActors\":0}">>
    end.

metrics() ->
    Counts = case whereis(?SERVER) of
        Pid when is_pid(Pid) ->
            try gen_server:call(Pid, counts, 2000) of
                Value -> Value
            catch
                _:_ -> #{active => 0, queued => 0}
            end;
        _ ->
            #{active => 0, queued => 0}
    end,
    iolist_to_binary([
        "# HELP dd_lambda_runner_active_actors Hot keyed BEAM actor processes.\n",
        "# TYPE dd_lambda_runner_active_actors gauge\n",
        metric_line("dd_lambda_runner_active_actors", maps:get(active, Counts, 0)),
        "# HELP dd_lambda_runner_actor_queued_calls Calls waiting in actor mailboxes.\n",
        "# TYPE dd_lambda_runner_actor_queued_calls gauge\n",
        metric_line("dd_lambda_runner_actor_queued_calls", maps:get(queued, Counts, 0)),
        counter_metric(
            "dd_lambda_runner_actor_invocations_total",
            "Durable actor request invocations.",
            invocations_total
        ),
        counter_metric(
            "dd_lambda_runner_actor_invocation_failures_total",
            "Durable actor requests that failed.",
            invocation_failures_total
        ),
        counter_metric(
            "dd_lambda_runner_actor_alarm_invocations_total",
            "Durable actor alarm handler attempts.",
            alarm_invocations_total
        ),
        counter_metric(
            "dd_lambda_runner_actor_alarm_failures_total",
            "Durable actor alarm handler failures.",
            alarm_failures_total
        ),
        counter_metric(
            "dd_lambda_runner_actor_queue_rejections_total",
            "Durable actor calls rejected at the bounded mailbox limit.",
            queue_rejections_total
        )
    ]).

init([]) ->
    ensure_metrics(),
    {ok, #{actors => #{}, refs => #{}}}.

handle_call({actor_pid, FunctionRef, ActorKey}, _From, State) ->
    Key = {FunctionRef, ActorKey},
    Actors = maps:get(actors, State),
    case maps:get(Key, Actors, undefined) of
        #{pid := Pid} when is_pid(Pid) ->
            case erlang:is_process_alive(Pid) of
                true ->
                    {reply, {ok, Pid}, State};
                false ->
                    start_actor(Key, drop_actor(Key, State))
            end;
        _ ->
            start_actor(Key, State)
    end;
handle_call(snapshot, _From, State) ->
    Counts = state_counts(State),
    Body = iolist_to_binary(json:encode(#{
        <<"ok">> => true,
        <<"dynamicChildren">> => true,
        <<"durableStorage">> => <<"postgres">>,
        <<"crossReplicaLease">> => true,
        <<"activeActors">> => maps:get(active, Counts),
        <<"queuedCalls">> => maps:get(queued, Counts)
    })),
    {reply, Body, State};
handle_call(counts, _From, State) ->
    {reply, state_counts(State), State};
handle_call(_Request, _From, State) ->
    {reply, {error, unsupported}, State}.

handle_cast(_Message, State) ->
    {noreply, State}.

handle_info({'DOWN', Ref, process, _Pid, _Reason}, State) ->
    Refs = maps:get(refs, State),
    case maps:take(Ref, Refs) of
        error ->
            {noreply, State};
        {Key, NewRefs} ->
            Actors = maps:remove(Key, maps:get(actors, State)),
            {noreply, State#{actors := Actors, refs := NewRefs}}
    end;
handle_info(_Message, State) ->
    {noreply, State}.

terminate(_Reason, _State) ->
    ok.

actor_pid(FunctionRef0, ActorKey0) ->
    FunctionRef = to_binary(FunctionRef0),
    ActorKey = to_binary(ActorKey0),
    case enabled() of
        false ->
            {error, <<"durable actor engine is unavailable">>};
        true ->
            case lambda_actor_store:valid_actor_key(ActorKey) of
                false -> {error, <<"actor key contains unsupported characters">>};
                true ->
                    case whereis(?SERVER) of
                        Pid when is_pid(Pid) ->
                            gen_server:call(
                                Pid,
                                {actor_pid, FunctionRef, ActorKey},
                                5000
                            );
                        _ ->
                            {error, <<"durable actor manager is unavailable">>}
                    end
            end
    end.

start_actor(Key = {FunctionRef, ActorKey}, State) ->
    Actors = maps:get(actors, State),
    MaxActive = env_int(
        <<"ACTOR_MAX_ACTIVE_PER_REPLICA">>,
        ?DEFAULT_MAX_ACTIVE,
        1,
        1000000
    ),
    case map_size(Actors) >= MaxActive of
        true ->
            {reply, {error, <<"actor process capacity reached">>}, State};
        false ->
            case lambda_actor_supervisor:start_actor(FunctionRef, ActorKey) of
                {ok, Pid} ->
                    Ref = erlang:monitor(process, Pid),
                    Actor = #{pid => Pid, ref => Ref},
                    NewActors = Actors#{Key => Actor},
                    NewRefs = (maps:get(refs, State))#{Ref => Key},
                    {reply, {ok, Pid}, State#{
                        actors := NewActors,
                        refs := NewRefs
                    }};
                {error, Reason} ->
                    {reply, {error, iolist_to_binary(io_lib:format(
                        "could not start durable actor: ~p",
                        [Reason]
                    ))}, State}
            end
    end.

drop_actor(Key, State) ->
    Actors = maps:get(actors, State),
    Refs0 = maps:get(refs, State),
    case maps:take(Key, Actors) of
        error ->
            State;
        {Actor, NewActors} ->
            Ref = maps:get(ref, Actor, undefined),
            case is_reference(Ref) of
                true -> erlang:demonitor(Ref, [flush]);
                false -> ok
            end,
            State#{
                actors := NewActors,
                refs := maps:remove(Ref, Refs0)
            }
    end.

safe_actor_call(Callback) ->
    try Callback() of
        {ok, _} = Success -> Success;
        {error, _} = Error ->
            bump(invocation_failures_total, 1),
            Error;
        Other ->
            bump(invocation_failures_total, 1),
            {error, iolist_to_binary(io_lib:format(
                "invalid durable actor reply: ~p",
                [Other]
            ))}
    catch
        exit:{timeout, _} ->
            bump(invocation_failures_total, 1),
            {error, <<"durable actor call timed out">>};
        exit:_ ->
            bump(invocation_failures_total, 1),
            {error, <<"durable actor process exited">>}
    end.

queue_available(Pid) ->
    Limit = env_int(<<"ACTOR_MAX_QUEUE_DEPTH">>, ?DEFAULT_MAX_QUEUE, 1, 10000),
    case process_info(Pid, message_queue_len) of
        {message_queue_len, Length} -> Length < Limit;
        _ -> false
    end.

state_counts(State) ->
    Actors = maps:get(actors, State),
    Queued = maps:fold(
        fun(_Key, Actor, Total) ->
            case process_info(maps:get(pid, Actor), message_queue_len) of
                {message_queue_len, Length} -> Total + Length;
                _ -> Total
            end
        end,
        0,
        Actors
    ),
    #{active => map_size(Actors), queued => Queued}.

call_timeout(TimeoutMs0) ->
    TimeoutMs = clamp_int(TimeoutMs0, 1000, 300000),
    QueueWaitMs = env_int(<<"ACTOR_QUEUE_WAIT_MS">>, 30000, 0, 300000),
    TimeoutMs + QueueWaitMs + 10000.

ensure_metrics() ->
    case ets:info(?METRICS) of
        undefined ->
            try ets:new(?METRICS, [
                named_table, public, set,
                {write_concurrency, true}
            ]) of
                _ -> ok
            catch
                error:badarg -> ok
            end;
        _ -> ok
    end.

bump(Name, Amount) ->
    ensure_metrics(),
    ets:update_counter(?METRICS, Name, Amount, {Name, 0}).

metric(Name) ->
    ensure_metrics(),
    case ets:lookup(?METRICS, Name) of
        [{Name, Value}] -> Value;
        _ -> 0
    end.

counter_metric(Name, Help, Key) ->
    [
        "# HELP ", Name, " ", Help, "\n",
        "# TYPE ", Name, " counter\n",
        metric_line(Name, metric(Key))
    ].

metric_line(Name, Value) ->
    io_lib:format("~s{service=\"dd-gleam-lambda-runner\"} ~p~n", [Name, Value]).

env_flag(Name, Default) ->
    case os:getenv(binary_to_list(Name)) of
        false -> Default;
        Value ->
            lists:member(
                string:lowercase(string:trim(Value)),
                ["1", "true", "yes", "on"]
            )
    end.

env_int(Name, Default, Min, Max) ->
    Value = case os:getenv(binary_to_list(Name)) of
        false -> Default;
        Raw ->
            case string:to_integer(Raw) of
                {Parsed, []} -> Parsed;
                _ -> Default
            end
    end,
    clamp_int(Value, Min, Max).

clamp_int(Value, Min, _Max) when is_integer(Value), Value < Min -> Min;
clamp_int(Value, _Min, Max) when is_integer(Value), Value > Max -> Max;
clamp_int(Value, _Min, _Max) when is_integer(Value) -> Value;
clamp_int(_Value, Min, _Max) -> Min.

to_binary(Value) when is_binary(Value) -> Value;
to_binary(Value) when is_list(Value) -> unicode:characters_to_binary(Value);
to_binary(Value) -> unicode:characters_to_binary(io_lib:format("~p", [Value])).
