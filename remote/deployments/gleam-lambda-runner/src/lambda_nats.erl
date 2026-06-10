-module(lambda_nats).

-export([start/0, publish/2, request/3, pool_dispatch/5]).

-define(SERVER, lambda_nats_singleton).
-define(DEFAULT_COMMAND, <<"env -i PATH=\"$PATH\" NODE_ENV=production NODE_NO_WARNINGS=1 NATS_URL=\"${NATS_URL:-}\" CONTAINER_POOL_NATS_URL=\"${CONTAINER_POOL_NATS_URL:-}\" CONTAINER_POOL_NATS_SUBJECT_PREFIX=\"${CONTAINER_POOL_NATS_SUBJECT_PREFIX:-dd.remote.container_pool}\" CONTAINER_POOL_NATS_TIMEOUT_MS=\"${CONTAINER_POOL_NATS_TIMEOUT_MS:-30000}\" node --permission --allow-net child-runtimes/js-function-runner.mjs">>).
-define(DEFAULT_IDLE_MS, 300000).
-define(DEFAULT_TIMEOUT_MS, 30000).

start() ->
    case dd_cli_config_client_ffi:getenv(<<"NATS_URL">>, <<>>) of
        <<>> ->
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

%% NATS request/reply: publish to Subject with a private `_INBOX` reply and
%% block the calling process until a single reply arrives or TimeoutMs elapses.
%% The socket-owning singleton routes the inbox reply back to the caller.
request(Subject0, Payload0, TimeoutMs) ->
    Subject = to_binary(Subject0),
    Payload = to_binary(Payload0),
    Timeout = max_int(TimeoutMs, 1),
    case {whereis(?SERVER), is_connected()} of
        {undefined, _} ->
            {error, <<"lambda nats is not configured (NATS_URL unset)">>};
        {_Pid, false} ->
            %% Fast-fail while disconnected so a pool outage falls back to local
            %% execution immediately instead of blocking the full request budget,
            %% and so request messages do not pile up in the singleton mailbox.
            {error, <<"lambda nats is not connected">>};
        {Pid, true} ->
            Ref = make_ref(),
            Pid ! {request, self(), Ref, Subject, Payload, Timeout},
            receive
                {Ref, Result} -> Result
            after Timeout + 2000 ->
                %% Belt-and-suspenders: the singleton also arms its own timer,
                %% but never block the caller longer than the agreed budget.
                {error, <<"container pool request timed out">>}
            end
    end.

%% Dispatch one lambda invocation to dd-container-pool over NATS request/reply.
%% Builds the DispatchRequest envelope the pool expects, then unwraps the
%% DispatchResponse `body` (success) or `error` (failure).
pool_dispatch(Subject0, PoolSlug0, RequestId0, PayloadJson0, TimeoutMs) ->
    Subject = to_binary(Subject0),
    PoolSlug = to_binary(PoolSlug0),
    RequestId = to_binary(RequestId0),
    PayloadJson = to_binary(PayloadJson0),
    case request(Subject, pool_request_envelope(PoolSlug, RequestId, PayloadJson), TimeoutMs) of
        {ok, ResponseJson} -> parse_pool_response(ResponseJson);
        {error, Reason} -> {error, Reason}
    end.

pool_request_envelope(PoolSlug, RequestId, PayloadJson) ->
    SlugField = case PoolSlug of
        <<>> -> [];
        _ -> ["\"poolSlug\":\"", json_escape(PoolSlug), "\","]
    end,
    iolist_to_binary([
        "{",
        SlugField,
        "\"requestId\":\"", json_escape(RequestId), "\",",
        "\"source\":\"dd-gleam-lambda-runner\",",
        "\"payload\":", pool_payload_value(PayloadJson),
        "}"
    ]).

%% The lambda request payload is already a normalized JSON value; embed it raw.
pool_payload_value(<<>>) -> <<"null">>;
pool_payload_value(Payload) -> Payload.

parse_pool_response(ResponseJson) ->
    case pool_response_ok(ResponseJson) of
        true ->
            case json_field_slice(ResponseJson, <<"body">>) of
                {ok, Body} -> {ok, Body};
                error -> {ok, ResponseJson}
            end;
        false ->
            case json_field_slice(ResponseJson, <<"error">>) of
                {ok, Error} -> {error, unwrap_json_string(Error)};
                error -> {error, ResponseJson}
            end
    end.

%% Anchor on the leading object key so a nested "ok":true inside the worker's
%% response body cannot flip our reading of the top-level dispatch outcome.
%% dd-container-pool serializes DispatchResponse with `ok` as the first field.
pool_response_ok(ResponseJson) ->
    case re:run(ResponseJson, "^[[:space:]]*\\{[[:space:]]*\"ok\"[[:space:]]*:[[:space:]]*true", [{capture, none}]) of
        match -> true;
        nomatch -> false
    end.

unwrap_json_string(Value) ->
    case Value of
        <<$", Rest/binary>> ->
            case byte_size(Rest) >= 1 andalso binary:last(Rest) =:= $" of
                true -> json_unescape_string(binary:part(Rest, 0, byte_size(Rest) - 1));
                false -> Value
            end;
        _ ->
            Value
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
    set_connected(false),
    %% NATS subject defaults below come from dd_nats_subject_consts which
    %% is auto-generated from remote/libs/nats/subject-defs/schema/
    %% lambdas.schema.json. A subject rename in the schema surfaces here
    %% as a build error (missing function) instead of silently drifting
    %% out of sync with the Rust/Gleam/Python/etc consumers.
    State = #{
        nats_url => env_binary("NATS_URL", <<>>),
        invoke_subject => env_binary("NATS_LAMBDA_INVOKE_SUBJECT", dd_nats_subject_consts:lambdas_invoke_wildcard()),
        queue_group => env_binary("NATS_LAMBDA_QUEUE_GROUP", dd_nats_subject_consts:lambda_runner_queue_group()),
        result_subject => env_binary("NATS_LAMBDA_RESULT_SUBJECT", dd_nats_subject_consts:lambdas_results_subject()),
        functions_subject => env_binary("NATS_LAMBDA_FUNCTIONS_SUBJECT", dd_nats_subject_consts:lambdas_functions_subject()),
        nats_username => env_binary("NATS_USERNAME", <<>>),
        nats_password => env_binary("NATS_PASSWORD", <<>>),
        nats_token => env_binary("NATS_TOKEN", <<>>),
        reconnect_ms => env_int("NATS_LAMBDA_RECONNECT_MS", 1000),
        max_payload_bytes => env_int("NATS_LAMBDA_MAX_PAYLOAD_BYTES", 5242880),
        buffer => <<>>,
        %% Outstanding request/reply correlations: InboxSubject => {From, Ref, Sid, TimerRef}.
        %% Sids 1 (invoke) and 2 (functions) are reserved by connect/1.
        inboxes => #{},
        next_sid => 3
    },
    connect(State).

connect(State) ->
    case parse_nats_url(maps:get(nats_url, State)) of
        {ok, Host, Port, UrlAuth} ->
            case gen_tcp:connect(Host, Port, [binary, {packet, raw}, {active, true}], 5000) of
                {ok, Socket} ->
                    %% A send failure here (socket already reset by the peer) must
                    %% reconnect, never crash this unsupervised singleton with an
                    %% `ok =` badmatch -- that would silently stop all NATS traffic
                    %% until the pod restarts.
                    case gen_tcp:send(Socket, connect_payload(nats_auth(State, UrlAuth))) of
                        ok ->
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
                            set_connected(true),
                            loop(State#{socket => Socket, buffer => <<>>});
                        {error, SendReason} ->
                            io:format("lambda nats handshake send failed: ~p~n", [SendReason]),
                            catch gen_tcp:close(Socket),
                            timer:sleep(maps:get(reconnect_ms, State)),
                            connect(State)
                    end;
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
            reconnect(State, <<"lambda nats connection closed">>);
        {tcp_error, Socket, Reason} ->
            io:format("lambda nats socket error: ~p~n", [Reason]),
            catch gen_tcp:close(Socket),
            reconnect(State, <<"lambda nats connection error">>);
        {publish, Subject, Payload} ->
            send_pub(Socket, Subject, Payload),
            loop(State);
        {request, From, Ref, Subject, Payload, TimeoutMs} ->
            loop(start_request(State, From, Ref, Subject, Payload, TimeoutMs));
        {request_timeout, Inbox} ->
            loop(expire_request(State, Inbox));
        stop ->
            set_connected(false),
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
                close ->
                    catch gen_tcp:close(Socket),
                    reconnect(State, <<"lambda nats reset after oversized message">>);
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
                    case ByteCount > maps:get(max_payload_bytes, State) of
                        true ->
                            io:format(
                                "lambda nats dropping oversized message subject=~s bytes=~p~n",
                                [Subject, ByteCount]
                            ),
                            close;
                        false ->
                            PayloadStart = HeaderEnd + 2,
                            FrameEnd = PayloadStart + ByteCount + 2,
                            case byte_size(Buffer) >= FrameEnd of
                                true ->
                                    Payload = binary:part(Buffer, PayloadStart, ByteCount),
                                    Rest = binary:part(Buffer, FrameEnd, byte_size(Buffer) - FrameEnd),
                                    {continue, route_message(State#{buffer => Rest}, Subject, ReplyTo, Payload)};
                                false ->
                                    wait
                            end
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

%% Runs in the socket-owning process so it can read/update the inbox registry.
%% A message on a registered inbox is a request/reply response: hand it to the
%% waiting caller and tear the subscription down. Everything else is a normal
%% invoke/functions message handled in a spawned process as before.
route_message(State = #{socket := Socket}, Subject, ReplyTo, Payload) ->
    Inboxes = maps:get(inboxes, State, #{}),
    case maps:take(Subject, Inboxes) of
        {{From, Ref, Sid, TimerRef}, Remaining} ->
            cancel_timer(TimerRef),
            send_unsub(Socket, Sid),
            From ! {Ref, {ok, Payload}},
            State#{inboxes => Remaining};
        error ->
            spawn(fun() -> handle_message(State, Subject, ReplyTo, Payload) end),
            State
    end.

start_request(State = #{socket := Socket}, From, Ref, Subject, Payload, TimeoutMs) ->
    Max = maps:get(max_payload_bytes, State, 5242880),
    case byte_size(Payload) > Max of
        true ->
            %% Refuse locally rather than let the NATS server reject an oversized
            %% PUB with -ERR and drop the shared connection, which would fail every
            %% other in-flight request/reply caller too.
            From ! {Ref, {error, <<"container pool request payload too large">>}},
            State;
        false ->
            Inbox = new_inbox(),
            Sid = maps:get(next_sid, State, 3),
            subscribe(Socket, Inbox, <<>>, Sid),
            send_request(Socket, Subject, Inbox, Payload),
            TimerRef = erlang:send_after(TimeoutMs, self(), {request_timeout, Inbox}),
            Inboxes = maps:get(inboxes, State, #{}),
            State#{
                inboxes => Inboxes#{Inbox => {From, Ref, Sid, TimerRef}},
                next_sid => Sid + 1
            }
    end.

expire_request(State, Inbox) ->
    Inboxes = maps:get(inboxes, State, #{}),
    case maps:take(Inbox, Inboxes) of
        {{From, Ref, Sid, _TimerRef}, Remaining} ->
            case maps:get(socket, State, undefined) of
                undefined -> ok;
                Socket -> send_unsub(Socket, Sid)
            end,
            From ! {Ref, {error, <<"container pool request timed out">>}},
            State#{inboxes => Remaining};
        error ->
            State
    end.

%% Mark the client disconnected, fail every outstanding request/reply caller so
%% none blocks for its full budget on a connection we already know is gone, then
%% reconnect with a clean inbox registry (stale sids must not be UNSUBbed on the
%% new socket).
reconnect(State, Reason) ->
    set_connected(false),
    CleanState = fail_all_inboxes(State, Reason),
    connect(maps:remove(socket, CleanState#{buffer => <<>>})).

fail_all_inboxes(State, Reason) ->
    Inboxes = maps:get(inboxes, State, #{}),
    maps:foreach(
        fun(_Inbox, {From, Ref, _Sid, TimerRef}) ->
            cancel_timer(TimerRef),
            From ! {Ref, {error, Reason}}
        end,
        Inboxes
    ),
    State#{inboxes => #{}}.

set_connected(Bool) ->
    persistent_term:put({?MODULE, connected}, Bool).

is_connected() ->
    persistent_term:get({?MODULE, connected}, false).

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

send_request(Socket, Subject, ReplyTo, Payload) ->
    gen_tcp:send(Socket, [
        "PUB ",
        Subject,
        " ",
        ReplyTo,
        " ",
        integer_to_binary(byte_size(Payload)),
        "\r\n",
        Payload,
        "\r\n"
    ]).

send_unsub(Socket, Sid) ->
    gen_tcp:send(Socket, ["UNSUB ", integer_to_binary(Sid), "\r\n"]).

new_inbox() ->
    iolist_to_binary(["_INBOX.", binary:encode_hex(crypto:strong_rand_bytes(12))]).

cancel_timer(TimerRef) ->
    _ = (catch erlang:cancel_timer(TimerRef)),
    ok.

max_int(Value, Min) when is_integer(Value), Value >= Min -> Value;
max_int(_Value, Min) -> Min.

connect_payload(Auth) ->
    iolist_to_binary([
        "CONNECT {\"verbose\":false,\"pedantic\":false,\"lang\":\"erlang\",\"version\":\"dd-gleam-lambda-runner\"",
        nats_auth_fields(Auth),
        "}\r\n"
    ]).

nats_auth(State, UrlAuth) ->
    case maps:get(nats_token, State) of
        <<>> ->
            User = maps:get(nats_username, State),
            Pass = maps:get(nats_password, State),
            case {User, Pass} of
                {<<>>, _} -> UrlAuth;
                {_, <<>>} -> UrlAuth;
                _ -> #{user => User, pass => Pass}
            end;
        Token ->
            #{token => Token}
    end.

nats_auth_fields(#{token := Token}) ->
    [",\"auth_token\":\"", json_escape(Token), "\""];
nats_auth_fields(#{user := User, pass := Pass}) ->
    [",\"user\":\"", json_escape(User), "\",\"pass\":\"", json_escape(Pass), "\""];
nats_auth_fields(_) ->
    [].

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
            {ok, Host, maps:get(port, Parsed, 4222), parse_nats_url_auth(maps:get(userinfo, Parsed, ""))};
        _ ->
            {error, Url0}
    catch
        _:_ -> {error, Url0}
    end.

parse_nats_url_auth("") ->
    #{};
parse_nats_url_auth(UserInfo0) ->
    UserInfo = to_binary(uri_string:percent_decode(UserInfo0)),
    case binary:match(UserInfo, <<":">>) of
        nomatch ->
            #{token => UserInfo};
        {Index, 1} ->
            User = binary:part(UserInfo, 0, Index),
            Pass = binary:part(UserInfo, Index + 1, byte_size(UserInfo) - Index - 1),
            #{user => User, pass => Pass}
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
    dd_cli_config_client_ffi:getenv(Name, Default).

env_int(Name, Default) ->
    Value = dd_cli_config_client_ffi:getenv(Name, <<>>),
    try binary_to_integer(Value)
    catch _:_ -> Default
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
    Return = binary:replace(Newline, <<"\r">>, <<"\\r">>, [global]),
    binary:replace(Return, <<"\t">>, <<"\\t">>, [global]).

json_unescape_string(Value0) ->
    Value1 = binary:replace(Value0, <<"\\\"">>, <<"\"">>, [global]),
    binary:replace(Value1, <<"\\\\">>, <<"\\">>, [global]).

to_binary(Value) when is_binary(Value) ->
    Value;
to_binary(Value) when is_list(Value) ->
    unicode:characters_to_binary(Value);
to_binary(Value) ->
    unicode:characters_to_binary(io_lib:format("~p", [Value])).

now_ms() ->
    erlang:system_time(millisecond).
