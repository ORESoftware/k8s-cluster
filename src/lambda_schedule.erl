%% Supervised UTC cron scheduler for stored lambda functions.
%%
%% Schedules live in lambda_functions.meta_data, so no runner-private schema is
%% introduced. Every due fire enters the Postgres-durable async invocation path
%% with a deterministic per-function/per-schedule/per-minute idempotency key.
%% All replicas may observe a due schedule; the database admits one run.
-module(lambda_schedule).
-behaviour(gen_server).

-export([
    start_link/0,
    enabled/0,
    metrics/0,
    cron_matches/2,
    due_events/2
]).

-export([init/1, handle_call/3, handle_cast/2, handle_info/2, terminate/2]).

-define(SERVER, ?MODULE).
-define(METRICS, lambda_schedule_metrics).
-define(MAX_SCHEDULES_PER_FUNCTION, 50).
-define(DEFAULT_MAX_DISPATCH_PER_MINUTE, 500).

start_link() ->
    gen_server:start_link({local, ?SERVER}, ?MODULE, [], []).

enabled() ->
    workflow_store:available() andalso env_flag(<<"SCHEDULE_ENGINE_ENABLED">>, true).

init([]) ->
    ensure_metrics(),
    self() ! tick,
    {ok, #{last_minute => undefined}}.

handle_call(_Request, _From, State) ->
    {reply, {error, unsupported}, State}.

handle_cast(_Message, State) ->
    {noreply, State}.

handle_info(tick, State) ->
    erlang:send_after(ms_until_next_minute(), self(), tick),
    Minute = erlang:system_time(second) div 60,
    case {enabled(), maps:get(last_minute, State, undefined) =:= Minute} of
        {true, false} ->
            DateTime = calendar:universal_time(),
            dispatch_minute(DateTime),
            {noreply, State#{last_minute => Minute}};
        _ ->
            {noreply, State}
    end;
handle_info(_Message, State) ->
    {noreply, State}.

terminate(_Reason, _State) ->
    ok.

dispatch_minute(DateTime) ->
    bump(schedule_scans_total, 1),
    case workflow_store:list_scheduled_functions() of
        {ok, Functions} ->
            Due = due_events(Functions, DateTime),
            Limit = env_int(
                <<"SCHEDULE_MAX_DISPATCH_PER_MINUTE">>,
                ?DEFAULT_MAX_DISPATCH_PER_MINUTE,
                1,
                5000
            ),
            Selected = lists:sublist(Due, Limit),
            bump(schedule_due_total, length(Due)),
            bump(schedule_overflow_total, max(0, length(Due) - length(Selected))),
            lists:foreach(
                fun(Event) -> spawn(fun() -> dispatch_event(Event, DateTime) end) end,
                Selected
            );
        {error, Reason} ->
            bump(schedule_scan_errors_total, 1),
            io:format("lambda schedule discovery failed: ~s~n", [safe(Reason)])
    end.

dispatch_event({Function, Schedule, Index}, DateTime) ->
    FunctionId = map_bin(Function, <<"id">>, <<>>),
    FunctionSlug = map_bin(Function, <<"slug">>, FunctionId),
    Name = schedule_name(Schedule, Index),
    MinuteKey = minute_key(DateTime),
    ScheduleToken = schedule_token(Name, Index),
    IdempotencyKey = iolist_to_binary([
        "cron:",
        ScheduleToken,
        ":",
        MinuteKey
    ]),
    EventId = iolist_to_binary([
        FunctionId,
        ":",
        ScheduleToken,
        ":",
        MinuteKey
    ]),
    CloudEvent = #{
        <<"specversion">> => <<"1.0">>,
        <<"id">> => EventId,
        <<"source">> => iolist_to_binary([
            "/functions/",
            FunctionId,
            "/schedules/",
            ScheduleToken
        ]),
        <<"type">> => <<"dev.scintilla.function.scheduled.v1">>,
        <<"subject">> => FunctionSlug,
        <<"time">> => iso_minute(DateTime),
        <<"datacontenttype">> => <<"application/json">>,
        <<"data">> => maps:get(<<"payload">>, Schedule, null),
        <<"schedule">> => Name
    },
    Request0 = #{
        <<"payload">> => CloudEvent,
        <<"idempotencyKey">> => IdempotencyKey,
        <<"retry">> => map_value(Schedule, <<"retry">>, #{})
    },
    Request1 = maybe_copy_int(
        Schedule,
        Request0,
        <<"maxEventAgeMs">>
    ),
    Request = maybe_copy_int(Schedule, Request1, <<"timeoutMs">>),
    case lambda_async:start_from_body(FunctionId, json:encode(Request)) of
        {ok, _RunJson} ->
            bump(schedule_dispatch_total, 1);
        {error, Reason} ->
            bump(schedule_dispatch_errors_total, 1),
            io:format(
                "lambda schedule dispatch failed function=~s schedule=~s reason=~s~n",
                [safe(FunctionId), safe(Name), safe(Reason)]
            )
    end.

due_events(Functions, DateTime) when is_list(Functions) ->
    lists:flatmap(
        fun(Function) -> due_function_events(Function, DateTime) end,
        Functions
    );
due_events(_Functions, _DateTime) ->
    [].

due_function_events(Function, DateTime) when is_map(Function) ->
    MetaData = map_value(Function, <<"metaData">>, #{}),
    Schedules = schedules_from_metadata(MetaData),
    lists:filtermap(
        fun({Schedule, Index}) ->
            case schedule_due(Schedule, DateTime) of
                true -> {true, {Function, Schedule, Index}};
                false -> false
            end
        end,
        lists:zip(Schedules, lists:seq(0, max(0, length(Schedules) - 1)))
    );
due_function_events(_Function, _DateTime) ->
    [].

schedules_from_metadata(MetaData) when is_map(MetaData) ->
    Declared = case maps:get(<<"schedules">>, MetaData, []) of
        Schedules when is_list(Schedules) ->
            lists:sublist(
                [Schedule || Schedule <- Schedules, is_map(Schedule)],
                ?MAX_SCHEDULES_PER_FUNCTION
            );
        _ ->
            []
    end,
    case {Declared, maps:get(<<"cron">>, MetaData, undefined)} of
        {[], Cron} when is_binary(Cron) ->
            [#{
                <<"name">> => <<"default">>,
                <<"cron">> => Cron,
                <<"payload">> => maps:get(<<"schedulePayload">>, MetaData, null)
            }];
        _ ->
            Declared
    end;
schedules_from_metadata(_MetaData) ->
    [].

schedule_due(Schedule, DateTime) ->
    Enabled = maps:get(<<"enabled">>, Schedule, true) =:= true,
    Timezone = map_bin(Schedule, <<"timezone">>, <<"UTC">>),
    Cron = map_bin(Schedule, <<"cron">>, <<>>),
    Enabled andalso
        lists:member(Timezone, [<<"UTC">>, <<"Etc/UTC">>, <<"Z">>]) andalso
        cron_matches(Cron, DateTime).

%% Five-field Vixie-style UTC cron. Numeric lists, ranges, and steps are
%% supported. When both day-of-month and day-of-week are restricted, their
%% standard OR semantics apply.
cron_matches(Expression0, {{Year, Month, Day}, {Hour, Minute, _Second}}) ->
    Expression = cron_alias(to_binary(Expression0)),
    case string:lexemes(binary_to_list(Expression), " \t") of
        [MinuteField, HourField, DayField, MonthField, WeekdayField] ->
            Weekday0 = calendar:day_of_the_week(Year, Month, Day),
            Weekday = case Weekday0 of
                7 -> 0;
                Value -> Value
            end,
            MinuteMatch = field_matches(MinuteField, Minute, 0, 59, identity),
            HourMatch = field_matches(HourField, Hour, 0, 23, identity),
            MonthMatch = field_matches(MonthField, Month, 1, 12, identity),
            DayMatch = field_matches(DayField, Day, 1, 31, identity),
            WeekdayMatch = field_matches(
                WeekdayField,
                Weekday,
                0,
                7,
                weekday
            ),
            CalendarDayMatch = case {
                field_starts_with_star(DayField),
                field_starts_with_star(WeekdayField)
            } of
                {true, true} -> true;
                {true, false} -> WeekdayMatch;
                {false, true} -> DayMatch;
                {false, false} -> DayMatch orelse WeekdayMatch
            end,
            MinuteMatch andalso HourMatch andalso MonthMatch andalso
                CalendarDayMatch;
        _ ->
            false
    end;
cron_matches(_Expression, _DateTime) ->
    false.

cron_alias(<<"@yearly">>) -> <<"0 0 1 1 *">>;
cron_alias(<<"@annually">>) -> <<"0 0 1 1 *">>;
cron_alias(<<"@monthly">>) -> <<"0 0 1 * *">>;
cron_alias(<<"@weekly">>) -> <<"0 0 * * 0">>;
cron_alias(<<"@daily">>) -> <<"0 0 * * *">>;
cron_alias(<<"@midnight">>) -> <<"0 0 * * *">>;
cron_alias(<<"@hourly">>) -> <<"0 * * * *">>;
cron_alias(Expression) -> Expression.

field_starts_with_star([$* | _]) -> true;
field_starts_with_star(_) -> false.

field_matches(Field, Value, Min, Max, Normalize) ->
    Segments = string:split(Field, ",", all),
    Segments =/= [] andalso
    not lists:member([], Segments) andalso lists:any(
        fun(Segment) ->
            segment_matches(Segment, Value, Min, Max, Normalize)
        end,
        Segments
    ).

segment_matches(Segment, Value, Min, Max, Normalize) ->
    case string:split(Segment, "/", all) of
        [Base] ->
            base_matches(Base, 1, Value, Min, Max, Normalize);
        [Base, StepRaw] ->
            case parse_int(StepRaw) of
                {ok, Step} when Step > 0 ->
                    base_matches(Base, Step, Value, Min, Max, Normalize);
                _ ->
                    false
            end;
        _ ->
            false
    end.

base_matches("*", Step, Value, Min, Max, Normalize) ->
    range_matches(Min, Max, Step, Value, Min, Max, Normalize);
base_matches(Base, Step, Value, Min, Max, Normalize) ->
    case string:split(Base, "-", all) of
        [StartRaw, EndRaw] ->
            case {parse_int(StartRaw), parse_int(EndRaw)} of
                {{ok, Start}, {ok, End}} ->
                    range_matches(Start, End, Step, Value, Min, Max, Normalize);
                _ ->
                    false
            end;
        [SingleRaw] ->
            case parse_int(SingleRaw) of
                {ok, Single} ->
                    range_matches(
                        Single,
                        Single,
                        Step,
                        Value,
                        Min,
                        Max,
                        Normalize
                    );
                _ ->
                    false
            end;
        _ ->
            false
    end.

range_matches(Start, End, Step, Value, Min, Max, Normalize)
    when Start >= Min, End =< Max, Start =< End, Step > 0 ->
    lists:any(
        fun(Candidate) -> normalize_value(Candidate, Normalize) =:= Value end,
        lists:seq(Start, End, Step)
    );
range_matches(_Start, _End, _Step, _Value, _Min, _Max, _Normalize) ->
    false.

normalize_value(7, weekday) -> 0;
normalize_value(Value, _Normalize) -> Value.

parse_int(Value) ->
    try list_to_integer(Value) of
        Int -> {ok, Int}
    catch
        _:_ -> error
    end.

schedule_name(Schedule, Index) ->
    case map_bin(Schedule, <<"name">>, <<>>) of
        <<>> -> iolist_to_binary(["schedule-", integer_to_binary(Index)]);
        Name -> Name
    end.

schedule_token(Name, Index) ->
    Sanitized = re:replace(
        Name,
        <<"[^A-Za-z0-9._-]">>,
        <<"-">>,
        [global, {return, binary}]
    ),
    Bounded = case byte_size(Sanitized) > 64 of
        true -> binary:part(Sanitized, 0, 64);
        false -> Sanitized
    end,
    iolist_to_binary([integer_to_binary(Index), "-", Bounded]).

minute_key({{Year, Month, Day}, {Hour, Minute, _Second}}) ->
    iolist_to_binary(io_lib:format(
        "~4..0B~2..0B~2..0BT~2..0B~2..0BZ",
        [Year, Month, Day, Hour, Minute]
    )).

iso_minute({{Year, Month, Day}, {Hour, Minute, _Second}}) ->
    iolist_to_binary(io_lib:format(
        "~4..0B-~2..0B-~2..0BT~2..0B:~2..0B:00Z",
        [Year, Month, Day, Hour, Minute]
    )).

maybe_copy_int(Source, Target, Key) ->
    case maps:get(Key, Source, undefined) of
        Value when is_integer(Value) -> Target#{Key => Value};
        _ -> Target
    end.

map_value(Map, Key, Default) when is_map(Map) ->
    maps:get(Key, Map, Default);
map_value(_Map, _Key, Default) ->
    Default.

map_bin(Map, Key, Default) ->
    case map_value(Map, Key, Default) of
        Value when is_binary(Value) -> Value;
        Value when is_list(Value) -> iolist_to_binary(Value);
        _ -> Default
    end.

ms_until_next_minute() ->
    Now = erlang:system_time(millisecond),
    60050 - (Now rem 60000).

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

metric(Key) ->
    try ets:lookup(?METRICS, Key) of
        [{Key, Value}] -> Value;
        _ -> 0
    catch
        _:_ -> 0
    end.

metrics() ->
    Counters = [
        {<<"lambda_schedule_scans_total">>, schedule_scans_total},
        {<<"lambda_schedule_scan_errors_total">>, schedule_scan_errors_total},
        {<<"lambda_schedule_due_total">>, schedule_due_total},
        {<<"lambda_schedule_dispatch_total">>, schedule_dispatch_total},
        {<<"lambda_schedule_dispatch_errors_total">>, schedule_dispatch_errors_total},
        {<<"lambda_schedule_overflow_total">>, schedule_overflow_total}
    ],
    iolist_to_binary([
        <<"# HELP lambda_schedule Supervised UTC cron scheduler counters\n">>,
        <<"# TYPE lambda_schedule_dispatch_total counter\n">>,
        [
            [Name, <<" ">>, integer_to_binary(metric(Key)), <<"\n">>]
         || {Name, Key} <- Counters
        ]
    ]).

env_flag(Name, Default) ->
    case getenv(Name) of
        <<>> -> Default;
        <<"0">> -> false;
        <<"false">> -> false;
        <<"no">> -> false;
        _ -> true
    end.

env_int(Name, Default, Min, Max) ->
    case getenv(Name) of
        <<>> ->
            Default;
        Bin ->
            try binary_to_integer(Bin) of
                Value when Value >= Min, Value =< Max -> Value;
                _ -> Default
            catch
                _:_ -> Default
            end
    end.

getenv(Name) ->
    case os:getenv(binary_to_list(Name)) of
        false -> <<>>;
        "" -> <<>>;
        Value -> list_to_binary(Value)
    end.

safe(Value) when is_binary(Value) ->
    binary_to_list(binary:replace(Value, <<"\n">>, <<" ">>, [global]));
safe(Value) when is_list(Value) ->
    safe(iolist_to_binary(Value));
safe(Value) ->
    safe(iolist_to_binary(io_lib:format("~p", [Value]))).

to_binary(Value) when is_binary(Value) -> Value;
to_binary(Value) when is_list(Value) -> iolist_to_binary(Value);
to_binary(Value) when is_atom(Value) -> atom_to_binary(Value, utf8);
to_binary(_) -> <<>>.
