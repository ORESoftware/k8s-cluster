%% One hot keyed actor. gen_server mailbox ordering gives every actor a
%% single-threaded local event loop; lambda_actor_store leases extend that
%% guarantee across runner replicas. This process is temporary and exits after
%% an idle period because all authoritative state is already in Postgres.
-module(lambda_actor).
-behaviour(gen_server).

-export([start_link/2, invoke/6, invoke_alarm/6, reset/4]).
-export([init/1, handle_call/3, handle_cast/2, handle_info/2, terminate/2]).

-define(DEFAULT_LEASE_MS, 310000).
-define(DEFAULT_QUEUE_WAIT_MS, 30000).
-define(DEFAULT_IDLE_MS, 60000).

start_link(FunctionRef, ActorKey) ->
    gen_server:start_link(?MODULE, [FunctionRef, ActorKey], []).

invoke(Pid, Command, Payload, ChildIdleMs, TimeoutMs, CallTimeout) ->
    gen_server:call(
        Pid,
        {invoke, request, Command, Payload, ChildIdleMs, TimeoutMs},
        CallTimeout
    ).

invoke_alarm(Pid, Command, ScheduledAt, ChildIdleMs, TimeoutMs, CallTimeout) ->
    Payload = iolist_to_binary(json:encode(#{
        <<"type">> => <<"alarm">>,
        <<"scheduledAt">> => ScheduledAt
    })),
    gen_server:call(
        Pid,
        {invoke, alarm, Command, Payload, ChildIdleMs, TimeoutMs},
        CallTimeout
    ).

reset(Pid, QueueWaitMs, LeaseMs, CallTimeout) ->
    gen_server:call(Pid, {reset, QueueWaitMs, LeaseMs}, CallTimeout).

init([FunctionRef0, ActorKey0]) ->
    FunctionRef = to_binary(FunctionRef0),
    ActorKey = to_binary(ActorKey0),
    Owner = owner_id(FunctionRef, ActorKey),
    IdleMs = env_int(<<"ACTOR_IDLE_MS">>, ?DEFAULT_IDLE_MS, 1000, 3600000),
    {ok, #{
        function_ref => FunctionRef,
        actor_key => ActorKey,
        owner => Owner,
        idle_ms => IdleMs
    }, IdleMs}.

handle_call(
    {invoke, Kind, Command0, Payload0, ChildIdleMs0, TimeoutMs0},
    _From,
    State
) ->
    Command = to_binary(Command0),
    Payload = default_json(Payload0),
    ChildIdleMs = clamp_int(ChildIdleMs0, 1000, 3600000),
    TimeoutMs = clamp_int(TimeoutMs0, 1000, 300000),
    LeaseMs = env_int(
        <<"ACTOR_LEASE_MS">>,
        max(?DEFAULT_LEASE_MS, TimeoutMs + 10000),
        TimeoutMs + 1000,
        360000
    ),
    QueueWaitMs = env_int(
        <<"ACTOR_QUEUE_WAIT_MS">>,
        ?DEFAULT_QUEUE_WAIT_MS,
        0,
        300000
    ),
    Reply = case claim_with_wait(State, LeaseMs, QueueWaitMs) of
        {ok, Claim} ->
            execute_claim(
                State,
                Claim,
                Kind,
                Command,
                Payload,
                ChildIdleMs,
                TimeoutMs
            );
        {error, Reason} ->
            {error, Reason}
    end,
    {reply, Reply, State, maps:get(idle_ms, State)};
handle_call({reset, QueueWaitMs0, LeaseMs0}, _From, State) ->
    QueueWaitMs = clamp_int(QueueWaitMs0, 0, 300000),
    LeaseMs = clamp_int(LeaseMs0, 1000, 360000),
    Reply = case claim_with_wait(State, LeaseMs, QueueWaitMs) of
        {ok, Claim} ->
            ActorId = map_binary(Claim, <<"actorId">>, <<>>),
            Version = map_integer(Claim, <<"stateVersion">>, -1),
            Owner = maps:get(owner, State),
            case lambda_actor_store:reset(
                ActorId,
                Owner,
                Version,
                <<"reset">>
            ) of
                {ok, Actor} ->
                    {ok, iolist_to_binary(json:encode(#{
                        <<"ok">> => true,
                        <<"actor">> => response_actor(Claim, Actor)
                    }))};
                {error, Reason} ->
                    {error, Reason}
            end;
        {error, Reason} ->
            {error, Reason}
    end,
    {reply, Reply, State, maps:get(idle_ms, State)};
handle_call(_Request, _From, State) ->
    {reply, {error, <<"unsupported actor operation">>}, State, maps:get(idle_ms, State)}.

handle_cast(_Message, State) ->
    {noreply, State, maps:get(idle_ms, State)}.

handle_info(timeout, State) ->
    {stop, normal, State};
handle_info(_Message, State) ->
    {noreply, State, maps:get(idle_ms, State)}.

terminate(_Reason, _State) ->
    ok.

claim_with_wait(State, LeaseMs, QueueWaitMs) ->
    Deadline = erlang:monotonic_time(millisecond) + QueueWaitMs,
    claim_until(State, LeaseMs, Deadline).

claim_until(State, LeaseMs, Deadline) ->
    FunctionRef = maps:get(function_ref, State),
    ActorKey = maps:get(actor_key, State),
    Owner = maps:get(owner, State),
    case lambda_actor_store:claim(FunctionRef, ActorKey, Owner, LeaseMs) of
        {ok, #{<<"status">> := <<"claimed">>} = Claim} ->
            {ok, Claim};
        {ok, #{<<"status">> := <<"notFound">>}} ->
            {error, <<"active lambda function not found">>};
        {ok, #{<<"status">> := <<"busy">>} = Busy} ->
            Remaining = Deadline - erlang:monotonic_time(millisecond),
            case Remaining > 0 of
                true ->
                    RetryAfter = map_integer(Busy, <<"retryAfterMs">>, 25),
                    timer:sleep(max(1, min(Remaining, min(RetryAfter, 250)))),
                    claim_until(State, LeaseMs, Deadline);
                false ->
                    {error, <<"actor concurrency wait limit reached">>}
            end;
        {ok, _Other} ->
            {error, <<"actor claim returned an invalid status">>};
        {error, Reason} ->
            {error, Reason}
    end.

execute_claim(State, Claim, Kind, Command, Payload, ChildIdleMs, TimeoutMs) ->
    ActorId = map_binary(Claim, <<"actorId">>, <<>>),
    FunctionId = map_binary(Claim, <<"functionId">>, <<>>),
    ActorKey = map_binary(Claim, <<"actorKey">>, maps:get(actor_key, State)),
    Version = map_integer(Claim, <<"stateVersion">>, -1),
    Definition = map_value(Claim, <<"definition">>, #{}),
    StoredAlarmAt = map_value(Claim, <<"alarmAt">>, null),
    AlarmAt = case Kind of
        alarm -> null;
        request -> StoredAlarmAt
    end,
    ActorContext = #{
        <<"id">> => ActorId,
        <<"key">> => ActorKey,
        <<"version">> => Version,
        <<"state">> => map_value(Claim, <<"state">>, #{}),
        <<"alarmAt">> => AlarmAt
    },
    DefinitionJson = iolist_to_binary(json:encode(Definition)),
    ActorJson = iolist_to_binary(json:encode(ActorContext)),
    Owner = maps:get(owner, State),
    case lambda_child_runner:invoke_actor_definition(
        Command,
        FunctionId,
        DefinitionJson,
        ActorJson,
        Payload,
        ChildIdleMs,
        TimeoutMs
    ) of
        {ok, Output} ->
            finish_child_output(
                Claim,
                Kind,
                Owner,
                ActorId,
                Version,
                Output
            );
        {error, Reason} ->
            _ = lambda_actor_store:fail(
                ActorId,
                Owner,
                Version,
                Reason,
                Kind =:= alarm
            ),
            {error, Reason}
    end.

finish_child_output(Claim, Kind, Owner, ActorId, Version, Output0) ->
    Output = to_binary(Output0),
    try json:decode(Output) of
        #{
            <<"ok">> := true,
            <<"actor">> := ActorMutation
        } = Child when is_map(ActorMutation) ->
            State = map_value(ActorMutation, <<"state">>, #{}),
            AlarmAt = map_value(ActorMutation, <<"alarmAt">>, null),
            case is_map(State) of
                true ->
                    case lambda_actor_store:commit(
                        ActorId,
                        Owner,
                        Version,
                        State,
                        AlarmAt,
                        kind_binary(Kind)
                    ) of
                        {ok, Committed} ->
                            InvocationId = map_value(Child, <<"invocationId">>, null),
                            Result = map_value(Child, <<"result">>, null),
                            {ok, iolist_to_binary(json:encode(#{
                                <<"ok">> => true,
                                <<"result">> => Result,
                                <<"invocationId">> => InvocationId,
                                <<"actor">> => response_actor(Claim, Committed)
                            }))};
                        {error, Reason} ->
                            _ = lambda_actor_store:fail(
                                ActorId,
                                Owner,
                                Version,
                                Reason,
                                Kind =:= alarm
                            ),
                            {error, Reason}
                    end;
                false ->
                    release_invalid_output(
                        ActorId,
                        Owner,
                        Version,
                        Kind,
                        <<"actor child returned non-object state">>
                    )
            end;
        #{<<"ok">> := false} = Child ->
            Reason = map_binary(Child, <<"error">>, <<"actor invocation failed">>),
            release_invalid_output(ActorId, Owner, Version, Kind, Reason);
        _ ->
            release_invalid_output(
                ActorId,
                Owner,
                Version,
                Kind,
                <<"actor child returned an invalid response">>
            )
    catch
        _:_ ->
            release_invalid_output(
                ActorId,
                Owner,
                Version,
                Kind,
                <<"actor child returned invalid JSON">>
            )
    end.

release_invalid_output(ActorId, Owner, Version, Kind, Reason) ->
    _ = lambda_actor_store:fail(
        ActorId,
        Owner,
        Version,
        Reason,
        Kind =:= alarm
    ),
    {error, Reason}.

response_actor(Claim, Committed) ->
    #{
        <<"id">> => map_value(Committed, <<"id">>, map_value(Claim, <<"actorId">>, null)),
        <<"functionId">> => map_value(Claim, <<"functionId">>, null),
        <<"key">> => map_value(Committed, <<"key">>, map_value(Claim, <<"actorKey">>, null)),
        <<"version">> => map_value(Committed, <<"version">>, map_value(Claim, <<"stateVersion">>, 0)),
        <<"alarmAt">> => map_value(Committed, <<"alarmAt">>, null)
    }.

kind_binary(alarm) -> <<"alarm">>;
kind_binary(request) -> <<"request">>.

owner_id(FunctionRef, ActorKey) ->
    Suffix = integer_to_binary(erlang:unique_integer([positive, monotonic])),
    Digest = binary:encode_hex(
        crypto:hash(sha256, <<FunctionRef/binary, 0, ActorKey/binary>>),
        lowercase
    ),
    Node = atom_to_binary(node(), utf8),
    clamp_binary(iolist_to_binary([Node, ":", Digest, ":", Suffix]), 200).

default_json(Value0) ->
    Value = trim(to_binary(Value0)),
    case Value of
        <<>> -> <<"null">>;
        _ -> Value
    end.

map_value(Map, Key, Default) when is_map(Map) -> maps:get(Key, Map, Default);
map_value(_, _Key, Default) -> Default.

map_binary(Map, Key, Default) ->
    case map_value(Map, Key, Default) of
        Value when is_binary(Value) -> Value;
        _ -> Default
    end.

map_integer(Map, Key, Default) ->
    case map_value(Map, Key, Default) of
        Value when is_integer(Value) -> Value;
        _ -> Default
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

clamp_binary(Value, MaxBytes) when byte_size(Value) =< MaxBytes -> Value;
clamp_binary(Value, MaxBytes) -> binary:part(Value, 0, MaxBytes).

trim(Value) ->
    unicode:characters_to_binary(string:trim(binary_to_list(Value))).

to_binary(Value) when is_binary(Value) -> Value;
to_binary(Value) when is_list(Value) -> unicode:characters_to_binary(Value);
to_binary(Value) -> unicode:characters_to_binary(io_lib:format("~p", [Value])).
