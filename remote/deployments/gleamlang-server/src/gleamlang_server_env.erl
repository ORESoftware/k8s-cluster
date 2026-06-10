-module(gleamlang_server_env).
-export([getenv/1, json_message_id/1, now_ms/0, publish_nats/1]).

getenv(Name) when is_binary(Name) ->
    dd_cli_config_client_ffi:env(Name).

now_ms() ->
    erlang:system_time(millisecond).

json_message_id(Payload) when is_binary(Payload) ->
    case re:run(
        Payload,
        <<"\"(?:messageId|message_id|id)\"\\s*:\\s*\"([^\"]{1,128})\"">>,
        [unicode, {capture, [1], binary}]
    ) of
        {match, [MessageId]} -> {ok, MessageId};
        _ -> {error, nil}
    end.

publish_nats(Payload) when is_binary(Payload) ->
    case dd_cli_config_client_ffi:env(<<"GLEAM_BROADCAST_SECRET">>) of
        {error, nil} ->
            {error, nil};
        {ok, <<>>} ->
            {error, nil};
        {ok, SecretBin} ->
            Secret = binary_to_list(SecretBin),
            Url = binary_to_list(
                dd_cli_config_client_ffi:getenv(
                    <<"GLEAM_NATS_PUBLISH_URL">>,
                    <<"http://127.0.0.1:8083/publish">>
                )
            ),
            %% NATS_PUBLISH_SUBJECT default comes from dd_nats_subject_consts
            %% (auto-generated from remote/libs/nats/subject-defs/schema/
            %% runtime-events.schema.json) so a schema rename surfaces at
            %% build time instead of silently drifting between Erlang FFI
            %% and the rest of the codebase.
            Subject = binary_to_list(
                dd_cli_config_client_ffi:getenv(
                    <<"NATS_PUBLISH_SUBJECT">>,
                    dd_nats_subject_consts:websocket_events_subject()
                )
            ),
            spawn(fun() -> post_nats_publish(Url, Secret, Subject, Payload) end),
            {ok, nil}
    end.

post_nats_publish(Url, Secret, Subject, Payload) ->
    case parse_http_url(Url) of
        {ok, Host, Port, Path} ->
            case gen_tcp:connect(Host, Port, [binary, {active, false}], 2000) of
                {ok, Socket} ->
                    Request = [
                        "POST ", Path, " HTTP/1.1\r\n",
                        "Host: ", Host, ":", integer_to_list(Port), "\r\n",
                        "Connection: close\r\n",
                        "Content-Type: text/plain; charset=utf-8\r\n",
                        "Content-Length: ", integer_to_list(byte_size(Payload)), "\r\n",
                        "x-dd-internal-auth: ", Secret, "\r\n",
                        "x-nats-subject: ", Subject, "\r\n",
                        "\r\n",
                        Payload
                    ],
                    ok = gen_tcp:send(Socket, Request),
                    case recv_status(Socket) of
                        {ok, Status} when Status >= 200, Status < 300 ->
                            ok;
                        {ok, Status} ->
                            io:format("gleam nats publish failed status=~p~n", [Status]);
                        {error, Reason} ->
                            io:format("gleam nats publish response failed: ~p~n", [Reason])
                    end,
                    gen_tcp:close(Socket);
                {error, Reason} ->
                    io:format("gleam nats publish connect failed: ~p~n", [Reason])
            end;
        {error, Reason} ->
            io:format("gleam nats publish invalid url: ~p~n", [Reason])
    end.

parse_http_url(Url) ->
    try uri_string:parse(Url) of
        #{scheme := "http", host := Host} = Parsed ->
            Path0 = maps:get(path, Parsed, "/"),
            Path = case maps:get(query, Parsed, undefined) of
                undefined -> Path0;
                Query -> Path0 ++ "?" ++ Query
            end,
            {ok, Host, maps:get(port, Parsed, 80), Path};
        _ ->
            {error, Url}
    catch
        _:_ -> {error, Url}
    end.

recv_status(Socket) ->
    case gen_tcp:recv(Socket, 0, 2000) of
        {ok, Response} ->
            case binary:split(Response, <<"\r\n">>) of
                [StatusLine | _] ->
                    case binary:split(StatusLine, <<" ">>, [global]) of
                        [_Http, StatusBin | _] ->
                            try {ok, binary_to_integer(StatusBin)}
                            catch _:_ -> {error, StatusLine}
                            end;
                        _ ->
                            {error, StatusLine}
                    end;
                _ ->
                    {error, Response}
            end;
        {error, Reason} ->
            {error, Reason}
    end.
