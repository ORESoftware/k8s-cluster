-module(dd_rust_network_mutex_client_ffi_helpers).

-export([getenv/1, emit_event/1, sleep_ms/1]).

getenv(Name0) ->
    Name = case Name0 of
        N when is_binary(N) -> binary_to_list(N);
        N when is_list(N) -> N
    end,
    case os:getenv(Name) of
        false -> {error, nil};
        "" -> {error, nil};
        Value -> {ok, list_to_binary(Value)}
    end.

emit_event(Line0) ->
    Line = case Line0 of
        L when is_binary(L) -> L;
        L when is_list(L) -> iolist_to_binary(L)
    end,
    io:put_chars([Line, $\n]),
    case io:get_line("") of
        "ack\n" -> {ok, nil};
        "ack" -> {ok, nil};
        <<"ack\n">> -> {ok, nil};
        <<"ack">> -> {ok, nil};
        eof -> {error, <<"expected ack from harness, got eof">>};
        Other ->
            {error, iolist_to_binary(io_lib:format("expected ack from harness, got ~p", [Other]))}
    end.

sleep_ms(Ms) ->
    timer:sleep(Ms),
    nil.
