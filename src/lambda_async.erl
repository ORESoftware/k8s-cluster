%% First-class durable asynchronous lambda invocation.
%%
%% This is a deliberately thin façade over the existing Postgres-backed
%% workflow engine. Each active lambda gets one internal, one-step workflow
%% definition. Runs therefore inherit atomic idempotency, cross-replica
%% FOR UPDATE SKIP LOCKED leasing, crash recovery, retry history, status, and
%% cancellation without introducing a second scheduler or private schema.
-module(lambda_async).

-export([
    start_from_body/2,
    start_qualified_from_body/4,
    get/1,
    cancel/1,
    %% Pure contract helper used by runtime tests.
    validate_request/1
]).

-define(MAX_IDEMPOTENCY_KEY_BYTES, 200).
-define(DEFAULT_MAX_ATTEMPTS, 3).
-define(DEFAULT_BACKOFF_MS, 1000).
-define(DEFAULT_BACKOFF_FACTOR, 2.0).
-define(DEFAULT_MAX_BACKOFF_MS, 60000).
-define(DEFAULT_MAX_EVENT_AGE_MS, 21600000).
-define(DEFAULT_TIMEOUT_MS, 30000).

start_from_body(FunctionRef0, Body0) ->
    case workflow_engine:enabled() of
        false ->
            {error, <<"durable async invocation is unavailable">>};
        true ->
            FunctionRef = to_binary(FunctionRef0),
            case validate_request(Body0) of
                {ok, Input, IdempotencyKey} ->
                    case workflow_store:ensure_async_definition(FunctionRef) of
                        {ok, DefinitionSlug} ->
                            workflow_engine:start_run(
                                DefinitionSlug,
                                Input,
                                IdempotencyKey
                            );
                        {error, Reason} ->
                            {error, Reason}
                    end;
                {error, Reason} ->
                    {error, Reason}
            end
    end.

start_qualified_from_body(FunctionRef0, Qualifier0, Affinity0, Body0) ->
    case workflow_engine:enabled() of
        false ->
            {error, <<"durable async invocation is unavailable">>};
        true ->
            FunctionRef = to_binary(FunctionRef0),
            Qualifier = to_binary(Qualifier0),
            Affinity = to_binary(Affinity0),
            case validate_request(Body0) of
                {ok, Input, IdempotencyKey} ->
                    case lambda_child_runner:resolve_qualified_reference(
                        FunctionRef,
                        Qualifier,
                        Affinity
                    ) of
                        {ok, PinnedFunctionRef} ->
                            start_pinned_async(
                                PinnedFunctionRef,
                                Input,
                                IdempotencyKey
                            );
                        {error, Reason} ->
                            {error, Reason}
                    end;
                {error, Reason} ->
                    {error, Reason}
            end
    end.

start_pinned_async(PinnedFunctionRef, Input0, IdempotencyKey) ->
    case binary:split(PinnedFunctionRef, <<"@">>) of
        [FunctionId, _Revision] ->
            case workflow_store:ensure_async_definition(FunctionId) of
                {ok, DefinitionSlug} ->
                    case pin_function_reference(Input0, PinnedFunctionRef) of
                        {ok, Input} ->
                            workflow_engine:start_run(
                                DefinitionSlug,
                                Input,
                                IdempotencyKey
                            );
                        {error, Reason} ->
                            {error, Reason}
                    end;
                {error, Reason} ->
                    {error, Reason}
            end;
        _ ->
            {error, <<"invalid pinned lambda function reference">>}
    end.

pin_function_reference(Input0, PinnedFunctionRef) ->
    Input = to_binary(Input0),
    try json:decode(Input) of
        #{<<"options">> := Options} = Decoded when is_map(Options) ->
            {ok, json:encode(Decoded#{
                <<"options">> => Options#{
                    <<"functionRef">> => PinnedFunctionRef
                }
            })};
        Decoded when is_map(Decoded) ->
            {ok, json:encode(Decoded#{
                <<"options">> => #{
                    <<"functionRef">> => PinnedFunctionRef
                }
            })}
    catch
        _:_ -> {error, <<"failed to pin async lambda revision">>}
    end.

validate_request(Body) ->
    case decode_request(Body) of
        {ok, Payload, Options, IdempotencyKey} ->
            {ok, json:encode(#{
                <<"payload">> => Payload,
                <<"options">> => Options
            }), IdempotencyKey};
        {error, Reason} ->
            {error, Reason}
    end.

get(RunId) ->
    async_run_result(workflow_engine:get_run(RunId)).

cancel(RunId) ->
    case async_run_result(workflow_engine:get_run(RunId)) of
        {ok, _RunJson} ->
            workflow_engine:cancel_run(RunId);
        {error, Reason} ->
            {error, Reason}
    end.

decode_request(Body0) ->
    Body = string:trim(to_binary(Body0)),
    case Body of
        <<>> ->
            {error, <<"request body is required">>};
        _ ->
            try json:decode(Body) of
                Request when is_map(Request) ->
                    Payload = maps:get(<<"payload">>, Request, null),
                    IdempotencyKey = to_binary(
                        maps:get(<<"idempotencyKey">>, Request, <<>>)
                    ),
                    case validate_idempotency_key(IdempotencyKey) of
                        ok ->
                            Retry = maps:get(<<"retry">>, Request, #{}),
                            case normalize_options(Request, Retry) of
                                {ok, Options} ->
                                    {ok, Payload, Options, IdempotencyKey};
                                {error, Reason} ->
                                    {error, Reason}
                            end;
                        {error, Reason} ->
                            {error, Reason}
                    end;
                _ ->
                    {error, <<"async invocation body must be a JSON object">>}
            catch
                _:_ -> {error, <<"invalid JSON body">>}
            end
    end.

normalize_options(Request, Retry) when is_map(Retry) ->
    with_int(Retry, <<"maxAttempts">>, ?DEFAULT_MAX_ATTEMPTS, 1, 1000,
        fun(MaxAttempts) ->
            with_int(Retry, <<"backoffMs">>, ?DEFAULT_BACKOFF_MS, 0, 86400000,
                fun(BackoffMs) ->
                    with_number(
                        Retry,
                        <<"backoffFactor">>,
                        ?DEFAULT_BACKOFF_FACTOR,
                        1.0,
                        100.0,
                        fun(BackoffFactor) ->
                            with_int(
                                Retry,
                                <<"maxBackoffMs">>,
                                ?DEFAULT_MAX_BACKOFF_MS,
                                0,
                                86400000,
                                fun(MaxBackoffMs) ->
                                    normalize_run_options(
                                        Request,
                                        #{
                                            <<"maxAttempts">> => MaxAttempts,
                                            <<"backoffMs">> => BackoffMs,
                                            <<"backoffFactor">> => BackoffFactor,
                                            <<"maxBackoffMs">> => MaxBackoffMs
                                        }
                                    )
                                end
                            )
                        end
                    )
                end
            )
        end
    );
normalize_options(_Request, _Retry) ->
    {error, <<"retry must be a JSON object">>}.

normalize_run_options(Request, Retry) ->
    with_int(
        Request,
        <<"maxEventAgeMs">>,
        ?DEFAULT_MAX_EVENT_AGE_MS,
        1000,
        604800000,
        fun(MaxEventAgeMs) ->
            with_int(
                Request,
                <<"timeoutMs">>,
                ?DEFAULT_TIMEOUT_MS,
                1000,
                300000,
                fun(TimeoutMs) ->
                    {ok, #{
                        <<"retry">> => Retry,
                        <<"maxEventAgeMs">> => MaxEventAgeMs,
                        <<"timeoutMs">> => TimeoutMs
                    }}
                end
            )
        end
    ).

with_int(Map, Key, Default, Min, Max, Next) ->
    case maps:get(Key, Map, Default) of
        Value when is_integer(Value), Value >= Min, Value =< Max ->
            Next(Value);
        Value when is_integer(Value) ->
            {error, iolist_to_binary([
                Key,
                " must be ",
                integer_to_binary(Min),
                "..=",
                integer_to_binary(Max)
            ])};
        _ ->
            {error, iolist_to_binary([Key, " must be an integer"])}
    end.

with_number(Map, Key, Default, Min, Max, Next) ->
    case maps:get(Key, Map, Default) of
        Value when is_number(Value), Value >= Min, Value =< Max ->
            Next(Value);
        Value when is_number(Value) ->
            {error, iolist_to_binary([
                Key,
                " must be within the supported range"
            ])};
        _ ->
            {error, iolist_to_binary([Key, " must be a number"])}
    end.

validate_idempotency_key(Key)
    when byte_size(Key) =< ?MAX_IDEMPOTENCY_KEY_BYTES ->
    ok;
validate_idempotency_key(_Key) ->
    {error, <<"idempotencyKey exceeds 200 bytes">>}.

async_run_result({ok, Json0}) ->
    Json = to_binary(Json0),
    try json:decode(Json) of
        #{<<"run">> := Run} = Envelope when is_map(Run) ->
            case async_definition(Run) of
                true -> {ok, iolist_to_binary(json:encode(Envelope))};
                false -> {error, <<"async invocation not found">>}
            end;
        Run when is_map(Run) ->
            case async_definition(Run) of
                true -> {ok, Json};
                false -> {error, <<"async invocation not found">>}
            end;
        _ ->
            {error, <<"async invocation not found">>}
    catch
        _:_ -> {error, <<"async invocation not found">>}
    end;
async_run_result({error, Reason}) ->
    {error, Reason}.

async_definition(Run) ->
    case maps:get(<<"definitionSlug">>, Run, <<>>) of
        <<"async-", _/binary>> -> true;
        _ -> false
    end.

to_binary(Value) when is_binary(Value) -> Value;
to_binary(Value) when is_list(Value) -> iolist_to_binary(Value);
to_binary(Value) when is_atom(Value) -> atom_to_binary(Value, utf8);
to_binary(Value) when is_integer(Value) -> integer_to_binary(Value);
to_binary(_) -> <<>>.
