%% Supervised CloudEvents 1.0 router.
%%
%% HTTP and NATS structured-mode events enter one bounded OTP router. Each
%% accepted routing job is a monitored Erlang process, so slow Postgres work
%% does not serialize unrelated events and a task failure cannot take down the
%% router. Matching targets enter the existing durable async engine; a stable
%% per-function/per-binding/per-event key collapses duplicate delivery across
%% transports, replicas, reconnects, and retries.
-module(lambda_events).
-behaviour(gen_server).

-export([
    start_link/0,
    enabled/0,
    route_from_body/1,
    metrics/0,
    %% Pure contract helpers used by runtime tests.
    validate_event/1,
    matching_targets/2,
    idempotency_key/5
]).

-export([init/1, handle_call/3, handle_cast/2, handle_info/2, terminate/2]).

-define(SERVER, ?MODULE).
-define(METRICS, lambda_events_metrics).
-define(DEFAULT_MAX_INFLIGHT, 64).
-define(DEFAULT_MAX_TARGETS, 100).
-define(MAX_BINDINGS_PER_FUNCTION, 50).
-define(MAX_ERRORS_IN_RESPONSE, 20).

start_link() ->
    gen_server:start_link({local, ?SERVER}, ?MODULE, [], []).

enabled() ->
    whereis(?SERVER) =/= undefined andalso workflow_engine:enabled().

route_from_body(Body) ->
    case validate_event(Body) of
        {ok, Event} ->
            bump(events_received_total, 1),
            case whereis(?SERVER) of
                undefined ->
                    bump(events_route_errors_total, 1),
                    {error, <<"CloudEvents router is unavailable">>};
                _Pid ->
                    %% A successfully persisted fan-out must not be reported as a
                    %% client timeout. The router's inflight bound is the wait
                    %% limit; callers may disconnect without cancelling durable
                    %% work that has already begun.
                    try gen_server:call(?SERVER, {route, Event}, infinity)
                    catch
                        exit:_ ->
                            bump(events_route_errors_total, 1),
                            {error, <<"CloudEvents router is unavailable">>}
                    end
            end;
        {error, Reason} ->
            bump(events_invalid_total, 1),
            {error, Reason}
    end.

init([]) ->
    ensure_metrics(),
    set_gauge(events_inflight, 0),
    {ok, #{
        jobs => #{},
        max_inflight => env_int(
            <<"EVENT_ROUTER_MAX_INFLIGHT">>,
            ?DEFAULT_MAX_INFLIGHT,
            1,
            1024
        )
    }}.

handle_call({route, Event}, From, State) ->
    Jobs = maps:get(jobs, State),
    case map_size(Jobs) >= maps:get(max_inflight, State) of
        true ->
            bump(events_backpressure_total, 1),
            {reply, {error, <<"CloudEvents router concurrency limit reached">>}, State};
        false ->
            Parent = self(),
            TaskRef = make_ref(),
            {_Pid, MonitorRef} = spawn_monitor(fun() ->
                Result = safe_route(Event),
                Parent ! {route_result, TaskRef, Result}
            end),
            bump(events_jobs_total, 1),
            bump(events_inflight, 1),
            {noreply, State#{
                jobs => Jobs#{TaskRef => {From, MonitorRef}}
            }}
    end;
handle_call(_Request, _From, State) ->
    {reply, {error, <<"unsupported CloudEvents router request">>}, State}.

handle_cast(_Message, State) ->
    {noreply, State}.

handle_info({route_result, TaskRef, Result}, State) ->
    Jobs = maps:get(jobs, State),
    case maps:take(TaskRef, Jobs) of
        {{From, MonitorRef}, Remaining} ->
            erlang:demonitor(MonitorRef, [flush]),
            bump(events_inflight, -1),
            gen_server:reply(From, Result),
            {noreply, State#{jobs => Remaining}};
        error ->
            {noreply, State}
    end;
handle_info({'DOWN', MonitorRef, process, _Pid, Reason}, State) ->
    case job_for_monitor(MonitorRef, maps:get(jobs, State)) of
        {ok, TaskRef, From} ->
            Jobs = maps:remove(TaskRef, maps:get(jobs, State)),
            bump(events_inflight, -1),
            bump(events_route_errors_total, 1),
            io:format("CloudEvents route task failed: ~p~n", [Reason]),
            gen_server:reply(From, {error, <<"CloudEvents route task failed">>}),
            {noreply, State#{jobs => Jobs}};
        error ->
            {noreply, State}
    end;
handle_info(_Message, State) ->
    {noreply, State}.

terminate(_Reason, State) ->
    maps:foreach(
        fun(_TaskRef, {From, MonitorRef}) ->
            erlang:demonitor(MonitorRef, [flush]),
            gen_server:reply(From, {error, <<"CloudEvents router restarted">>})
        end,
        maps:get(jobs, State, #{})
    ),
    set_gauge(events_inflight, 0),
    ok.

safe_route(Event) ->
    try route_event(Event)
    catch
        Class:Reason:Stack ->
            bump(events_route_errors_total, 1),
            io:format(
                "CloudEvents route crashed class=~p reason=~p stack=~p~n",
                [Class, Reason, Stack]
            ),
            {error, <<"CloudEvents route failed">>}
    end.

route_event(Event) ->
    case workflow_store:list_event_bound_functions() of
        {ok, Functions} ->
            Limit = env_int(
                <<"EVENT_ROUTER_MAX_TARGETS">>,
                ?DEFAULT_MAX_TARGETS,
                1,
                1000
            ),
            {Selected, Matched} = matching_targets_limited(
                Functions,
                Event,
                Limit
            ),
            Overflow = max(0, Matched - length(Selected)),
            bump(events_matched_total, Matched),
            bump(events_overflow_total, Overflow),
            case Overflow > 0 of
                true ->
                    %% Never partially fan out an event. The producer can retry
                    %% after raising the operator cap or narrowing bindings,
                    %% without wondering which subset was silently discarded.
                    {error, iolist_to_binary([
                        <<"CloudEvent target limit exceeded: matched ">>,
                        integer_to_binary(Matched),
                        <<", maximum ">>,
                        integer_to_binary(Limit)
                    ])};
                false ->
                    {Accepted, Errors} = dispatch_targets(Selected, Event),
                    bump(events_dispatched_total, Accepted),
                    bump(events_dispatch_errors_total, length(Errors)),
                    Summary = #{
                        <<"ok">> => true,
                        <<"eventId">> => maps:get(<<"id">>, Event),
                        <<"matched">> => Matched,
                        <<"accepted">> => Accepted,
                        <<"failed">> => length(Errors),
                        <<"overflow">> => 0,
                        <<"errors">> =>
                            lists:sublist(Errors, ?MAX_ERRORS_IN_RESPONSE)
                    },
                    {ok, iolist_to_binary(json:encode(Summary))}
            end;
        {error, Reason} ->
            bump(events_route_errors_total, 1),
            {error, Reason}
    end.

dispatch_targets(Targets, Event) ->
    lists:foldl(
        fun(Target, {Accepted, Errors}) ->
            case dispatch_target(Target, Event) of
                ok ->
                    {Accepted + 1, Errors};
                {error, Error} ->
                    {Accepted, [Error | Errors]}
            end
        end,
        {0, []},
        Targets
    ).

dispatch_target({Function, Binding, Index}, Event) ->
    FunctionId = map_binary(Function, <<"id">>, <<>>),
    BindingToken = binding_token(Binding, Index),
    EventId = maps:get(<<"id">>, Event),
    Source = maps:get(<<"source">>, Event),
    Type = maps:get(<<"type">>, Event),
    Request0 = #{
        <<"payload">> => Event,
        <<"idempotencyKey">> =>
            idempotency_key(FunctionId, BindingToken, EventId, Source, Type),
        <<"retry">> => maps:get(<<"retry">>, Binding, #{})
    },
    Request1 = maybe_copy(Binding, Request0, <<"maxEventAgeMs">>),
    Request = maybe_copy(Binding, Request1, <<"timeoutMs">>),
    case lambda_async:start_from_body(FunctionId, json:encode(Request)) of
        {ok, _RunJson} ->
            ok;
        {error, Reason} ->
            {error, #{
                <<"functionId">> => FunctionId,
                <<"binding">> => BindingToken,
                <<"error">> => clamp_binary(Reason, 1000)
            }}
    end.

%% Decode and validate CloudEvents structured content mode. The data value is
%% deliberately unconstrained JSON and is delivered to the function as part of
%% the complete canonical event envelope.
validate_event(Body0) ->
    Body = string:trim(to_binary(Body0)),
    case Body of
        <<>> ->
            {error, <<"CloudEvent body is required">>};
        _ ->
            try json:decode(Body) of
                Event when is_map(Event) ->
                    validate_required_fields(Event);
                _ ->
                    {error, <<"CloudEvent body must be a JSON object">>}
            catch
                _:_ -> {error, <<"invalid CloudEvent JSON body">>}
            end
    end.

validate_required_fields(Event) ->
    case maps:get(<<"specversion">>, Event, undefined) of
        <<"1.0">> ->
            case required_text(Event, <<"id">>, 4096) of
                ok ->
                    case required_text(Event, <<"source">>, 4096) of
                        ok ->
                            case required_text(Event, <<"type">>, 4096) of
                                ok -> validate_optional_fields(Event);
                                Error -> Error
                            end;
                        Error -> Error
                    end;
                Error -> Error
            end;
        _ ->
            {error, <<"CloudEvent specversion must be \"1.0\"">>}
    end.

validate_optional_fields(Event) ->
    Optional = [<<"subject">>, <<"time">>, <<"datacontenttype">>, <<"dataschema">>],
    case lists:all(fun(Key) -> optional_text(Event, Key, 4096) end, Optional) of
        true -> {ok, Event};
        false -> {error, <<"CloudEvent optional context attributes must be strings">>}
    end.

required_text(Map, Key, MaxBytes) ->
    case maps:get(Key, Map, undefined) of
        Value when is_binary(Value), byte_size(Value) > 0,
            byte_size(Value) =< MaxBytes ->
            ok;
        _ ->
            {error, iolist_to_binary([
                <<"CloudEvent ">>, Key, <<" must be a non-empty string">>
            ])}
    end.

optional_text(Map, Key, MaxBytes) ->
    case maps:find(Key, Map) of
        error -> true;
        {ok, Value} when is_binary(Value), byte_size(Value) =< MaxBytes -> true;
        _ -> false
    end.

%% Return one target for every matching binding. Multiple bindings on the same
%% function intentionally create separate logical deliveries, as they do in
%% Eventarc, Azure Functions, and Knative.
matching_targets(Functions, Event) when is_list(Functions), is_map(Event) ->
    lists:flatmap(
        fun(Function) -> function_targets(Function, Event) end,
        Functions
    );
matching_targets(_Functions, _Event) ->
    [].

%% The discovery query can return 5,000 functions with 50 bindings each. Keep
%% the route-time target list at the configured dispatch cap while still
%% counting all matches for overflow telemetry; a maliciously broad binding
%% must not allocate a 250,000-element fan-out list per inflight event.
matching_targets_limited(Functions, Event, Limit) ->
    {SelectedReverse, Matched} = lists:foldl(
        fun(Function, {Selected0, Count0}) ->
            lists:foldl(
                fun(Target, {Selected1, Count1}) ->
                    case Count1 < Limit of
                        true -> {[Target | Selected1], Count1 + 1};
                        false -> {Selected1, Count1 + 1}
                    end
                end,
                {Selected0, Count0},
                function_targets(Function, Event)
            )
        end,
        {[], 0},
        Functions
    ),
    {lists:reverse(SelectedReverse), Matched}.

function_targets(Function, Event) when is_map(Function) ->
    MetaData = maps:get(<<"metaData">>, Function, #{}),
    Bindings0 = case MetaData of
        Map when is_map(Map) -> maps:get(<<"eventBindings">>, Map, []);
        _ -> []
    end,
    Bindings = case Bindings0 of
        List when is_list(List) ->
            lists:sublist(
                [Binding || Binding <- List, is_map(Binding)],
                ?MAX_BINDINGS_PER_FUNCTION
            );
        _ ->
            []
    end,
    lists:filtermap(
        fun({Binding, Index}) ->
            case binding_matches(Binding, Event) of
                true -> {true, {Function, Binding, Index}};
                false -> false
            end
        end,
        index_list(Bindings)
    );
function_targets(_Function, _Event) ->
    [].

index_list(List) ->
    index_list(List, 0).

index_list([], _Index) ->
    [];
index_list([Value | Rest], Index) ->
    [{Value, Index} | index_list(Rest, Index + 1)].

binding_matches(Binding, Event) ->
    maps:get(<<"enabled">>, Binding, true) =:= true andalso
        optional_exact(Binding, <<"type">>, Event, <<"type">>) andalso
        optional_prefix(Binding, <<"typePrefix">>, Event, <<"type">>) andalso
        optional_exact(Binding, <<"source">>, Event, <<"source">>) andalso
        optional_prefix(Binding, <<"sourcePrefix">>, Event, <<"source">>) andalso
        optional_exact(Binding, <<"subject">>, Event, <<"subject">>) andalso
        attributes_match(maps:get(<<"attributes">>, Binding, undefined), Event).

optional_exact(Filter, FilterKey, Event, EventKey) ->
    case maps:find(FilterKey, Filter) of
        error ->
            true;
        {ok, Expected} when is_binary(Expected) ->
            maps:get(EventKey, Event, undefined) =:= Expected;
        _ ->
            false
    end.

optional_prefix(Filter, FilterKey, Event, EventKey) ->
    case maps:find(FilterKey, Filter) of
        error ->
            true;
        {ok, Prefix} when is_binary(Prefix) ->
            has_prefix(maps:get(EventKey, Event, undefined), Prefix);
        _ ->
            false
    end.

attributes_match(undefined, _Event) ->
    true;
attributes_match(Attributes, Event) when is_map(Attributes) ->
    maps:fold(
        fun(Key, Expected, Matches) ->
            Matches andalso maps:find(Key, Event) =:= {ok, Expected}
        end,
        true,
        Attributes
    );
attributes_match(_Attributes, _Event) ->
    false.

idempotency_key(FunctionId0, BindingToken0, EventId0, Source0, Type0) ->
    Digest = crypto:hash(sha256, iolist_to_binary([
        to_binary(FunctionId0), 0,
        to_binary(BindingToken0), 0,
        to_binary(EventId0), 0,
        to_binary(Source0), 0,
        to_binary(Type0)
    ])),
    <<"event:", (binary:encode_hex(Digest))/binary>>.

binding_token(Binding, Index) ->
    case map_binary(Binding, <<"name">>, <<>>) of
        <<>> ->
            integer_to_binary(Index);
        Name ->
            %% The index keeps duplicate names as distinct logical bindings.
            %% Including the name keeps the key diagnosable and stable when
            %% unrelated fields change in place.
            iolist_to_binary([Name, <<":">>, integer_to_binary(Index)])
    end.

maybe_copy(Source, Target, Key) ->
    case maps:find(Key, Source) of
        {ok, Value} -> Target#{Key => Value};
        error -> Target
    end.

map_binary(Map, Key, Default) ->
    case maps:get(Key, Map, Default) of
        Value when is_binary(Value) -> Value;
        Value when is_list(Value) -> iolist_to_binary(Value);
        _ -> Default
    end.

has_prefix(Value, Prefix) when is_binary(Value), is_binary(Prefix) ->
    Size = byte_size(Prefix),
    byte_size(Value) >= Size andalso binary:part(Value, 0, Size) =:= Prefix;
has_prefix(_Value, _Prefix) ->
    false.

job_for_monitor(MonitorRef, Jobs) ->
    maps:fold(
        fun(TaskRef, {From, Candidate}, Acc) ->
            case Acc of
                error when Candidate =:= MonitorRef -> {ok, TaskRef, From};
                _ -> Acc
            end
        end,
        error,
        Jobs
    ).

ensure_metrics() ->
    case ets:info(?METRICS) of
        undefined ->
            try ets:new(?METRICS, [named_table, public, set, {write_concurrency, true}])
            catch
                error:badarg -> ?METRICS
            end;
        _ ->
            ?METRICS
    end.

bump(Key, Amount) ->
    try ets:update_counter(?METRICS, Key, Amount, {Key, 0})
    catch
        _:_ -> 0
    end.

set_gauge(Key, Value) ->
    try ets:insert(?METRICS, {Key, Value})
    catch
        _:_ -> false
    end.

metric(Key) ->
    try ets:lookup(?METRICS, Key) of
        [{Key, Value}] -> Value;
        _ -> 0
    catch
        _:_ -> 0
    end.

metrics() ->
    Counters = [
        {<<"lambda_events_received_total">>, events_received_total},
        {<<"lambda_events_invalid_total">>, events_invalid_total},
        {<<"lambda_events_jobs_total">>, events_jobs_total},
        {<<"lambda_events_matched_total">>, events_matched_total},
        {<<"lambda_events_dispatched_total">>, events_dispatched_total},
        {<<"lambda_events_dispatch_errors_total">>, events_dispatch_errors_total},
        {<<"lambda_events_route_errors_total">>, events_route_errors_total},
        {<<"lambda_events_backpressure_total">>, events_backpressure_total},
        {<<"lambda_events_overflow_total">>, events_overflow_total},
        {<<"lambda_events_inflight">>, events_inflight}
    ],
    iolist_to_binary([
        <<"# HELP lambda_events Supervised CloudEvents router counters and gauge\n">>,
        <<"# TYPE lambda_events_inflight gauge\n">>,
        [
            [Name, <<" ">>, integer_to_binary(metric(Key)), <<"\n">>]
         || {Name, Key} <- Counters
        ]
    ]).

env_int(Name, Default, Min, Max) ->
    case os:getenv(binary_to_list(Name)) of
        false ->
            Default;
        "" ->
            Default;
        Value ->
            try list_to_integer(Value) of
                Parsed when Parsed >= Min, Parsed =< Max -> Parsed;
                _ -> Default
            catch
                _:_ -> Default
            end
    end.

clamp_binary(Value0, MaxBytes) ->
    Value = to_binary(Value0),
    case byte_size(Value) =< MaxBytes of
        true -> Value;
        false -> binary:part(Value, 0, MaxBytes)
    end.

to_binary(Value) when is_binary(Value) -> Value;
to_binary(Value) when is_list(Value) -> iolist_to_binary(Value);
to_binary(Value) when is_atom(Value) -> atom_to_binary(Value, utf8);
to_binary(Value) when is_integer(Value) -> integer_to_binary(Value);
to_binary(_) -> <<>>.
