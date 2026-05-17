-module(lambda_nats).

-export([start/0, publish/2]).

-define(SERVER, lambda_nats_singleton).
-define(DEFAULT_COMMAND, <<"env -i PATH=\"$PATH\" NODE_ENV=production node --permission --allow-net child-runtimes/js-function-runner.mjs">>).
-define(DEFAULT_IDLE_MS, 300000).
-define(DEFAULT_TIMEOUT_MS, 30000).

start() ->
    case os:getenv("NATS_URL") of
        false ->
            io:format("lambda nats disabled: NATS_URL is not configured~n"),
            nil;
        "" ->
            io:format("lambda nats disabled: NATS_URL is not configured~n"),
            nil;
        _Url ->
            ensure_started(),
            nil
    end.

publish(Subject0, Payload0) ->
    Subject = to_binary(Subject0),
    Payload = to_binary(Payload0),
    case whereis(?SERVER) of
        undefined -> {error, nil};
        Pid ->
            Pid ! {publish, Subject, Payload},
            {ok, nil}
    end.

ensure_started() ->
    case whereis(?SERVER) of
        undefined ->
            Pid = spawn(fun init/0),
            case catch register(?SERVER, Pid) of
                true -> ok;
                {'EXIT', _Reason} -> Pid ! stop
            end;
        _Pid ->
            ok
    end.

init() ->
    State = #{
        nats_url => env_binary("NATS_URL", <<>>),
        invoke_subject => env_binary("NATS_LAMBDA_INVOKE_SUBJECT", <<"dd.remote.lambdas.invoke.*">>),
        queue_group => env_binary("NATS_LAMBDA_QUEUE_GROUP", <<"dd-gleam-lambda-runner">>),
        result_subject => env_binary("NATS_LAMBDA_RESULT_SUBJECT", <<"dd.remote.lambdas.results">>),
        functions_subject => env_binary("NATS_LAMBDA_FUNCTIONS_SUBJECT", <<"dd.remote.lambdas.functions">>),
        reconnect_ms => env_int("NATS_LAMBDA_RECONNECT_MS", 1000),
        buffer => <<>>
    },
    connect(State).

connect(State) ->
    case parse_nats_url(maps:get(nats_url, State)) of
        {ok, Host, Port} ->
            case gen_tcp:connect(Host, Port, [binary, {packet, raw}, {active, true}], 5000) of
                {ok, Socket} ->
                    ok = gen_tcp:send(Socket, <<"CONNECT {\"verbose\":false,\"pedantic\":false,\"lang\":\"erlang\",\"version\":\"dd-gleam-lambda-runner\"}\r\n">>),
                    subscribe(Socket, maps:get(invoke_subject, State), maps:get(queue_group, State), 1),
                    subscribe(Socket, maps:get(functions_subject, State), <<>>, 2),
                    io:format(
                        "lambda nats connected invoke=~s functions=~s result=~s~n",
                        [
                            maps:get(invoke_subject, State),
                            maps:get(functions_subject, State),
                            maps:get(result_subject, State)
                        ]
                    ),
                    loop(State#{socket => Socket, buffer => <<>>});
                {error, Reason} ->
                    io:format("lambda nats connect failed: ~p~n", [Reason]),
                    timer:sleep(maps:get(reconnect_ms, State)),
                    connect(State)
            end;
        {error, Reason} ->
            io:format("lambda nats invalid NATS_URL: ~p~n", [Reason]),
            timer:sleep(maps:get(reconnect_ms, State)),
            connect(State)
    end.

subscribe(Socket, Subject, <<>>, Sid) ->
    gen_tcp:send(Socket, ["SUB ", Subject, " ", integer_to_binary(Sid), "\r\n"]);
subscribe(Socket, Subject, QueueGroup, Sid) ->
    gen_tcp:send(Socket, [
        "SUB ",
        Subject,
        " ",
        QueueGroup,
        " ",
        integer_to_binary(Sid),
        "\r\n"
    ]).

loop(State = #{socket := Socket, buffer := Buffer}) ->
    receive
        {tcp, Socket, Data} ->
            drain(State#{buffer => <<Buffer/binary, Data/binary>>});
        {tcp_closed, Socket} ->
            io:format("lambda nats socket closed; reconnecting~n"),
            connect(maps:remove(socket, State#{buffer => <<>>}));
        {tcp_error, Socket, Reason} ->
            io:format("lambda nats socket error: ~p~n", [Reason]),
            catch gen_tcp:close(Socket),
            connect(maps:remove(socket, State#{buffer => <<>>}));
        {publish, Subject, Payload} ->
            send_pub(Socket, Subject, Payload),
            loop(State);
        stop ->
            catch gen_tcp:close(Socket),
            ok;
        _Other ->
            loop(State)
    end.

drain(State = #{socket := Socket, buffer := Buffer}) ->
    case Buffer of
        <<>> ->
            loop(State);
        <<"PING\r\n", Rest/binary>> ->
            gen_tcp:send(Socket, <<"PONG\r\n">>),
            drain(State#{buffer => Rest});
        <<"MSG ", _/binary>> ->
            case drain_message(State) of
                {continue, NextState} -> drain(NextState);
                wait -> loop(State)
            end;
        _ ->
            case binary:match(Buffer, <<"\r\n">>) of
                {Index, 2} ->
                    Line = binary:part(Buffer, 0, Index),
                    Rest = binary:part(Buffer, Index + 2, byte_size(Buffer) - Index - 2),
                    log_nats_line(Line),
                    drain(State#{buffer => Rest});
                nomatch ->
                    loop(State)
            end
    end.

drain_message(State = #{buffer := Buffer}) ->
    case binary:match(Buffer, <<"\r\n">>) of
        nomatch ->
            wait;
        {HeaderEnd, 2} ->
            Header = binary:part(Buffer, 0, HeaderEnd),
            Parts = binary:split(Header, <<" ">>, [global]),
            case parse_msg_header(Parts) of
                {ok, Subject, ReplyTo, ByteCount} ->
                    PayloadStart = HeaderEnd + 2,
                    FrameEnd = PayloadStart + ByteCount + 2,
                    case byte_size(Buffer) >= FrameEnd of
                        true ->
                            Payload = binary:part(Buffer, PayloadStart, ByteCount),
                            Rest = binary:part(Buffer, FrameEnd, byte_size(Buffer) - FrameEnd),
                            spawn(fun() -> handle_message(State, Subject, ReplyTo, Payload) end),
                            {continue, State#{buffer => Rest}};
                        false ->
                            wait
                    end;
                error ->
                    Rest = binary:part(Buffer, HeaderEnd + 2, byte_size(Buffer) - HeaderEnd - 2),
                    {continue, State#{buffer => Rest}}
            end
    end.

parse_msg_header([<<"MSG">>, Subject, _Sid, Bytes]) ->
    parse_msg_header(Subject, undefined, Bytes);
parse_msg_header([<<"MSG">>, Subject, _Sid, ReplyTo, Bytes]) ->
    parse_msg_header(Subject, ReplyTo, Bytes);
parse_msg_header(_Parts) ->
    error.

parse_msg_header(Subject, ReplyTo, Bytes) ->
    case safe_binary_to_integer(Bytes) of
        {ok, Count} when Count >= 0 -> {ok, Subject, ReplyTo, Count};
        _ -> error
    end.

handle_message(State, Subject, ReplyTo, Payload) ->
    InvokeSubject = maps:get(invoke_subject, State),
    FunctionsSubject = maps:get(functions_subject, State),
    case subject_matches(Subject, InvokeSubject) of
        true ->
            handle_invoke(State, Subject, ReplyTo, Payload);
        false ->
            case subject_matches(Subject, FunctionsSubject) of
                true -> handle_function_update(State, Payload);
                false -> ok
            end
    end.

handle_invoke(State, Subject, ReplyTo, Payload0) ->
    InvokeSubject = maps:get(invoke_subject, State),
    case function_id_from_message(Subject, InvokeSubject, Payload0) of
        {ok, FunctionId} ->
            Payload = request_payload(Payload0),
            StartedAt = now_ms(),
            Result = lambda_child_runner:invoke(
                ?DEFAULT_COMMAND,
                FunctionId,
                Payload,
                ?DEFAULT_IDLE_MS,
                ?DEFAULT_TIMEOUT_MS
            ),
            DurationMs = now_ms() - StartedAt,
            ResultSubject = result_subject(State, ReplyTo),
            publish(ResultSubject, invocation_result_json(FunctionId, DurationMs, Result));
        {error, Reason} ->
            publish(result_subject(State, ReplyTo), invocation_error_json(Reason))
    end.

handle_function_update(State, Payload) ->
    publish(
        maps:get(result_subject, State),
        iolist_to_binary([
            "{\"type\":\"lambda-function-update-seen\",\"source\":\"dd-gleam-lambda-runner\",\"receivedAtMs\":",
            integer_to_binary(now_ms()),
            ",\"message\":\"",
            json_escape(Payload),
            "\"}"
        ])
    ).

function_id_from_message(Subject, InvokeSubject, Payload) ->
    case function_id_from_subject(Subject, InvokeSubject) of
        {ok, FunctionId} -> {ok, FunctionId};
        error -> function_id_from_payload(Payload)
    end.

function_id_from_subject(Subject, InvokeSubject) ->
    case binary:match(InvokeSubject, <<"*">>) of
        {StarIndex, 1} ->
            Prefix = binary:part(InvokeSubject, 0, StarIndex),
            case has_prefix(Subject, Prefix) of
                true ->
                    Tail0 = binary:part(Subject, StarIndex, byte_size(Subject) - StarIndex),
                    Tail = trim_dot(Tail0),
                    case Tail of
                        <<>> -> error;
                        _ -> {ok, Tail}
                    end;
                false ->
                    error
            end;
        nomatch ->
            error
    end.

function_id_from_payload(Payload) ->
    Pattern = <<"\"(functionId|function_id|slug|id)\"[[:space:]]*:[[:space:]]*\"([^\"]+)\"">>,
    case re:run(Payload, Pattern, [{capture, [2], binary}]) of
        {match, [FunctionId]} -> {ok, FunctionId};
        nomatch -> {error, <<"lambda invoke message requires function id in subject or payload">>}
    end.

request_payload(Payload) ->
    case json_field_slice(Payload, <<"payload">>) of
        {ok, Value} -> Value;
        error ->
            case json_field_slice(Payload, <<"request">>) of
                {ok, Value} -> Value;
                error -> Payload
            end
    end.

json_field_slice(Payload, Field) ->
    Pattern = iolist_to_binary(["\"", Field, "\"[[:space:]]*:[[:space:]]*"]),
    case re:run(Payload, Pattern, [{capture, first, index}]) of
        {match, [{Start, Length}]} ->
            ValueStart = Start + Length,
            slice_json_value(Payload, ValueStart);
        nomatch ->
            error
    end.

slice_json_value(Payload, Start) ->
    case scan_json_value(Payload, Start) of
        {ok, End} -> {ok, binary:part(Payload, Start, End - Start)};
        error -> error
    end.

scan_json_value(Payload, Start) when Start >= byte_size(Payload) ->
    error;
scan_json_value(Payload, Start) ->
    case binary:at(Payload, Start) of
        $" -> scan_json_string(Payload, Start + 1);
        ${ -> scan_json_composite(Payload, Start + 1, 1, $}, none);
        $[ -> scan_json_composite(Payload, Start + 1, 1, $], none);
        _ -> scan_json_scalar(Payload, Start)
    end.

scan_json_string(Payload, Pos) when Pos >= byte_size(Payload) ->
    {ok, byte_size(Payload)};
scan_json_string(Payload, Pos) ->
    case binary:at(Payload, Pos) of
        $\\ -> scan_json_string(Payload, Pos + 2);
        $" -> {ok, Pos + 1};
        _ -> scan_json_string(Payload, Pos + 1)
    end.

scan_json_composite(Payload, Pos, _Depth, _Close, _Mode) when Pos >= byte_size(Payload) ->
    {ok, byte_size(Payload)};
scan_json_composite(Payload, Pos, Depth, Close, string) ->
    case binary:at(Payload, Pos) of
        $\\ -> scan_json_composite(Payload, Pos + 2, Depth, Close, string);
        $" -> scan_json_composite(Payload, Pos + 1, Depth, Close, none);
        _ -> scan_json_composite(Payload, Pos + 1, Depth, Close, string)
    end;
scan_json_composite(Payload, Pos, Depth, Close, none) ->
    case binary:at(Payload, Pos) of
        $" -> scan_json_composite(Payload, Pos + 1, Depth, Close, string);
        ${ -> scan_json_composite(Payload, Pos + 1, Depth + 1, Close, none);
        $[ -> scan_json_composite(Payload, Pos + 1, Depth + 1, Close, none);
        Char when Char =:= Close, Depth =:= 1 -> {ok, Pos + 1};
        $} -> scan_json_composite(Payload, Pos + 1, Depth - 1, Close, none);
        $] -> scan_json_composite(Payload, Pos + 1, Depth - 1, Close, none);
        _ -> scan_json_composite(Payload, Pos + 1, Depth, Close, none)
    end.

scan_json_scalar(Payload, Pos) when Pos >= byte_size(Payload) ->
    {ok, byte_size(Payload)};
scan_json_scalar(Payload, Pos) ->
    case binary:at(Payload, Pos) of
        $, -> {ok, Pos};
        $} -> {ok, Pos};
        $] -> {ok, Pos};
        $\r -> {ok, Pos};
        $\n -> {ok, Pos};
        _ -> scan_json_scalar(Payload, Pos + 1)
    end.

result_subject(State, undefined) -> maps:get(result_subject, State);
result_subject(_State, ReplyTo) -> ReplyTo.

invocation_result_json(FunctionId, DurationMs, {ok, Output}) ->
    iolist_to_binary([
        "{\"type\":\"lambda-invocation-result\",\"ok\":true,\"functionId\":\"",
        json_escape(FunctionId),
        "\",\"durationMs\":",
        integer_to_binary(DurationMs),
        ",\"emittedAtMs\":",
        integer_to_binary(now_ms()),
        ",\"output\":\"",
        json_escape(Output),
        "\"}"
    ]);
invocation_result_json(FunctionId, DurationMs, {error, Error}) ->
    iolist_to_binary([
        "{\"type\":\"lambda-invocation-result\",\"ok\":false,\"functionId\":\"",
        json_escape(FunctionId),
        "\",\"durationMs\":",
        integer_to_binary(DurationMs),
        ",\"emittedAtMs\":",
        integer_to_binary(now_ms()),
        ",\"error\":\"",
        json_escape(Error),
        "\"}"
    ]).

invocation_error_json(Reason) ->
    iolist_to_binary([
        "{\"type\":\"lambda-invocation-result\",\"ok\":false,\"emittedAtMs\":",
        integer_to_binary(now_ms()),
        ",\"error\":\"",
        json_escape(Reason),
        "\"}"
    ]).

send_pub(Socket, Subject, Payload) ->
    gen_tcp:send(Socket, [
        "PUB ",
        Subject,
        " ",
        integer_to_binary(byte_size(Payload)),
        "\r\n",
        Payload,
        "\r\n"
    ]).

subject_matches(Subject, Pattern) ->
    case binary:match(Pattern, <<"*">>) of
        nomatch -> Subject =:= Pattern;
        {StarIndex, 1} ->
            Prefix = binary:part(Pattern, 0, StarIndex),
            has_prefix(Subject, Prefix)
    end.

parse_nats_url(Url0) ->
    Url = binary_to_list(to_binary(Url0)),
    try uri_string:parse(Url) of
        #{scheme := "nats", host := Host} = Parsed ->
            {ok, Host, maps:get(port, Parsed, 4222)};
        _ ->
            {error, Url0}
    catch
        _:_ -> {error, Url0}
    end.

log_nats_line(<<"-ERR", _/binary>> = Line) ->
    io:format("lambda nats server error: ~s~n", [Line]);
log_nats_line(_Line) ->
    ok.

safe_binary_to_integer(Value) ->
    try {ok, binary_to_integer(Value)}
    catch _:_ -> error
    end.

env_binary(Name, Default) ->
    case os:getenv(Name) of
        false -> Default;
        "" -> Default;
        Value -> unicode:characters_to_binary(Value)
    end.

env_int(Name, Default) ->
    case os:getenv(Name) of
        false -> Default;
        "" -> Default;
        Value ->
            try list_to_integer(Value)
            catch _:_ -> Default
            end
    end.

has_prefix(Value, Prefix) ->
    Size = byte_size(Prefix),
    byte_size(Value) >= Size andalso binary:part(Value, 0, Size) =:= Prefix.

trim_dot(<<$., Rest/binary>>) -> trim_dot(Rest);
trim_dot(Value) -> Value.

json_escape(Value0) ->
    Value = to_binary(Value0),
    Slash = binary:replace(Value, <<"\\">>, <<"\\\\">>, [global]),
    Quote = binary:replace(Slash, <<"\"">>, <<"\\\"">>, [global]),
    Newline = binary:replace(Quote, <<"\n">>, <<"\\n">>, [global]),
    binary:replace(Newline, <<"\r">>, <<"\\r">>, [global]).

to_binary(Value) when is_binary(Value) ->
    Value;
to_binary(Value) when is_list(Value) ->
    unicode:characters_to_binary(Value);
to_binary(Value) ->
    unicode:characters_to_binary(io_lib:format("~p", [Value])).

now_ms() ->
    erlang:system_time(millisecond).
