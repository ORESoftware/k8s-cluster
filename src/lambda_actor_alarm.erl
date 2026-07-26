%% Supervised scanner for per-actor durable alarms. Every replica may discover
%% the same due row; the actor's Postgres lease admits one handler at a time.
%% A successful commit clears/replaces the alarm, while failures are rescheduled
%% by lambda_actor_store with bounded exponential backoff.
-module(lambda_actor_alarm).
-behaviour(gen_server).

-export([start_link/1]).
-export([init/1, handle_call/3, handle_cast/2, handle_info/2, terminate/2]).

-define(DEFAULT_POLL_MS, 1000).
-define(DEFAULT_MAX_CONCURRENCY, 50).

start_link(Command) ->
    gen_server:start_link({local, ?MODULE}, ?MODULE, [Command], []).

init([Command0]) ->
    self() ! tick,
    {ok, #{
        command => to_binary(Command0),
        active => #{},
        keys => #{},
        cooldowns => #{},
        last_scan_error_log_ms => -60000
    }}.

handle_call(_Request, _From, State) ->
    {reply, {error, unsupported}, State}.

handle_cast(_Message, State) ->
    {noreply, State}.

handle_info(tick, State) ->
    PollMs = env_int(
        <<"ACTOR_ALARM_POLL_MS">>,
        ?DEFAULT_POLL_MS,
        100,
        60000
    ),
    erlang:send_after(PollMs, self(), tick),
    {noreply, dispatch_due(prune_cooldowns(State))};
handle_info({'DOWN', Ref, process, _Pid, Reason}, State) ->
    Active = maps:get(active, State),
    case maps:take(Ref, Active) of
        error ->
            {noreply, State};
        {Key, NewActive} ->
            Cooldowns = case Reason of
                normal -> maps:get(cooldowns, State);
                _ ->
                    CooldownMs = env_int(
                        <<"ACTOR_ALARM_ERROR_COOLDOWN_MS">>,
                        5000,
                        100,
                        300000
                    ),
                    (maps:get(cooldowns, State))#{
                        Key => erlang:monotonic_time(millisecond) + CooldownMs
                    }
            end,
            {noreply, State#{
                active := NewActive,
                keys := maps:remove(Key, maps:get(keys, State)),
                cooldowns := Cooldowns
            }}
    end;
handle_info(_Message, State) ->
    {noreply, State}.

terminate(_Reason, _State) ->
    ok.

dispatch_due(State) ->
    case lambda_actor_manager:enabled() of
        false ->
            State;
        true ->
            Active = maps:get(active, State),
            MaxConcurrency = env_int(
                <<"ACTOR_ALARM_MAX_CONCURRENCY">>,
                ?DEFAULT_MAX_CONCURRENCY,
                1,
                1000
            ),
            Capacity = max(0, MaxConcurrency - map_size(Active)),
            case Capacity of
                0 ->
                    State;
                _ ->
                    case lambda_actor_store:list_due_alarms(Capacity) of
                        {ok, Due} -> start_due(Due, State);
                        {error, Reason} -> maybe_log_scan_error(Reason, State)
                    end
            end
    end.

start_due(Due, State) ->
    lists:foldl(
        fun(Event, Current) ->
            FunctionId = map_binary(Event, <<"functionId">>, <<>>),
            ActorKey = map_binary(Event, <<"actorKey">>, <<>>),
            ScheduledAt = map_value(Event, <<"alarmAt">>, null),
            Key = {FunctionId, ActorKey},
            Keys = maps:get(keys, Current),
            CoolingDown = maps:is_key(Key, maps:get(cooldowns, Current)),
            case FunctionId =/= <<>> andalso ActorKey =/= <<>> andalso
                not maps:is_key(Key, Keys) andalso not CoolingDown of
                true ->
                    Command = maps:get(command, Current),
                    {Pid, Ref} = spawn_monitor(fun() ->
                        run_alarm(Command, FunctionId, ActorKey, ScheduledAt)
                    end),
                    _ = Pid,
                    Current#{
                        active := (maps:get(active, Current))#{Ref => Key},
                        keys := Keys#{Key => Ref}
                    };
                false ->
                    Current
            end
        end,
        State,
        Due
    ).

run_alarm(Command, FunctionId, ActorKey, ScheduledAt) ->
    IdleMs = env_int(<<"ACTOR_IDLE_MS">>, 60000, 1000, 3600000),
    TimeoutMs = env_int(<<"ACTOR_ALARM_TIMEOUT_MS">>, 300000, 1000, 300000),
    case lambda_actor_manager:invoke_alarm(
        Command,
        FunctionId,
        ActorKey,
        ScheduledAt,
        IdleMs,
        TimeoutMs
    ) of
        {ok, _} -> ok;
        {error, Reason} ->
            io:format(
                "durable actor alarm failed function=~s actor=~s reason=~s~n",
                [safe(FunctionId), safe(ActorKey), safe(Reason)]
            ),
            exit({alarm_failed, Reason})
    end.

prune_cooldowns(State) ->
    Now = erlang:monotonic_time(millisecond),
    Cooldowns = maps:filter(
        fun(_Key, Until) -> Until > Now end,
        maps:get(cooldowns, State)
    ),
    State#{cooldowns := Cooldowns}.

maybe_log_scan_error(Reason, State) ->
    Now = erlang:monotonic_time(millisecond),
    Last = maps:get(last_scan_error_log_ms, State),
    Interval = env_int(
        <<"ACTOR_ALARM_ERROR_LOG_INTERVAL_MS">>,
        30000,
        1000,
        3600000
    ),
    case Now - Last >= Interval of
        true ->
            io:format(
                "durable actor alarm scan failed: ~s~n",
                [safe(Reason)]
            ),
            State#{last_scan_error_log_ms := Now};
        false ->
            State
    end.

map_value(Map, Key, Default) when is_map(Map) -> maps:get(Key, Map, Default);
map_value(_, _Key, Default) -> Default.

map_binary(Map, Key, Default) ->
    case map_value(Map, Key, Default) of
        Value when is_binary(Value) -> Value;
        _ -> Default
    end.

safe(Value0) ->
    Value = to_binary(Value0),
    Sanitized = binary:replace(
        binary:replace(Value, <<"\n">>, <<" ">>, [global]),
        <<"\r">>,
        <<" ">>,
        [global]
    ),
    case byte_size(Sanitized) =< 500 of
        true -> Sanitized;
        false -> binary:part(Sanitized, 0, 500)
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
