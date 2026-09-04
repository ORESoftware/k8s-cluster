%%% =====================================================================
%%% Minimal NATS client.
%%%
%%% Why custom: pgo (Postgres) pins `opentelemetry_api ~> 1.5`; the
%%% community `enats` package pins it to `1.4.0`. We need NATS for
%%% parallel pub/sub broadcasts and PG for the membership store, so we
%%% can't pick. NATS' wire protocol is small enough that a focused
%%% implementation is cheaper than vendoring + patching enats.
%%%
%%% What we implement (subset of NATS 1.x):
%%%   - TCP connect + INFO/CONNECT handshake (no TLS, no JetStream).
%%%   - SUB / PUB / HPUB + MSG / HMSG dispatch.
%%%   - PING / PONG keepalive (server-driven; we also send PING on idle).
%%%   - Auto reconnect with jittered exponential backoff (1s → 30s).
%%%   - Subject sanitisation (reject CR/LF/space — protocol injection).
%%%   - Bounded read buffer / payload size (reconnect on overflow).
%%%   - CONNECT credentials JSON-escaped.
%%%   - Subscriptions replayed only AFTER the INFO/CONNECT handshake.
%%%
%%% Public API (called from Gleam via FFI):
%%%   start_link(Url, Notify)
%%%   publish(Pid, Subject, Payload, Headers)
%%%   subscribe(Pid, Subject)         — {ok, Sid} | {error, Reason}
%%%   unsubscribe(Pid, Sid)
%%%   valid_subject/1, parse_url_host/1, json_escape/1  (tests)
%%% =====================================================================
-module(dd_nats).

-behaviour(gen_server).

-export([
    start_link/2,
    publish/4,
    subscribe/2,
    unsubscribe/2,
    stop/1,
    valid_subject/1,
    parse_url_host/1,
    json_escape/1,
    header_get/2
]).

-export([
    init/1,
    handle_call/3,
    handle_cast/2,
    handle_info/2,
    terminate/2,
    code_change/3
]).

-define(RECONNECT_MIN_MS, 1000).
-define(RECONNECT_MAX_MS, 30000).
-define(PING_INTERVAL_MS, 30000).
-define(MAX_BUF_BYTES, 8388608).
-define(MAX_PAYLOAD_BYTES, 8388608).
-define(MAX_SUBS, 10000).
-define(MAX_SUBJECT_BYTES, 255).

-record(state, {
    url :: binary(),
    host :: string(),
    port :: pos_integer(),
    user :: undefined | string(),
    pass :: undefined | string(),
    sock = undefined :: undefined | gen_tcp:socket(),
    buf = <<>> :: binary(),
    notify :: pid(),
    notify_mon :: reference(),
    %% Pending subscriptions to (re-)send on (re)connect. sid → subject.
    subs = #{} :: #{integer() => binary()},
    next_sid = 1 :: pos_integer(),
    backoff_ms = ?RECONNECT_MIN_MS :: pos_integer(),
    ping_ref = undefined :: undefined | reference(),
    handshake = pending :: pending | ready
}).

%%% ---------------------------------------------------------------------
%%% Public API
%%% ---------------------------------------------------------------------

start_link(Url, Notify) ->
    gen_server:start_link(?MODULE, [Url, Notify], []).

publish(Pid, Subject, Payload, Headers) ->
    gen_server:cast(Pid, {publish, Subject, Payload, Headers}).

subscribe(Pid, Subject) ->
    gen_server:call(Pid, {subscribe, Subject}).

unsubscribe(Pid, Sid) ->
    gen_server:cast(Pid, {unsubscribe, Sid}).

stop(Pid) ->
    gen_server:stop(Pid).

%% NATS subjects are space-separated on the wire. CR/LF/NUL/tab would
%% inject a new protocol verb. Wildcards `*` and `>` are allowed.
valid_subject(Sub) when is_binary(Sub) ->
    Size = byte_size(Sub),
    Size > 0
        andalso Size =< ?MAX_SUBJECT_BYTES
        andalso binary:match(Sub, [<<" ">>, <<"\r">>, <<"\n">>, <<"\t">>, <<0>>]) =:= nomatch;
valid_subject(_) ->
    false.

parse_url_host(Url) ->
    case parse_url(Url) of
        {ok, Host, Port, _User, _Pass} ->
            {ok, {unicode:characters_to_binary(Host), Port}};
        {error, Reason} when is_atom(Reason) ->
            {error, atom_to_binary(Reason, utf8)};
        {error, Reason} when is_binary(Reason) ->
            {error, Reason};
        {error, Reason} ->
            {error, list_to_binary(io_lib:format("~p", [Reason]))}
    end.

json_escape(Bin) when is_binary(Bin) ->
    escape_json_bin(Bin);
json_escape(List) when is_list(List) ->
    escape_json_bin(unicode:characters_to_binary(List)).

header_get(Headers, Key) when is_list(Headers), is_binary(Key) ->
    Want = string:lowercase(Key),
    case first_header_ci(Headers, Want) of
        undefined -> {error, nil};
        Val -> {ok, Val}
    end.

%%% ---------------------------------------------------------------------
%%% gen_server callbacks
%%% ---------------------------------------------------------------------

init([Url, Notify]) when is_pid(Notify) ->
    process_flag(trap_exit, true),
    Mon = erlang:monitor(process, Notify),
    case parse_url(Url) of
        {ok, Host, Port, User, Pass} ->
            self() ! connect,
            {ok, schedule_ping(#state{
                url = Url,
                host = Host,
                port = Port,
                user = User,
                pass = Pass,
                notify = Notify,
                notify_mon = Mon
            })};
        {error, Reason} ->
            erlang:demonitor(Mon, [flush]),
            {stop, {bad_nats_url, Reason}}
    end;
init(_) ->
    {stop, badarg}.

handle_call({subscribe, Subject}, _From, State0) ->
    case valid_subject(Subject) of
        false ->
            {reply, {error, bad_subject}, State0};
        true ->
            case maps:size(State0#state.subs) >= ?MAX_SUBS of
                true ->
                    {reply, {error, too_many_subs}, State0};
                false ->
                    Sid = State0#state.next_sid,
                    State = State0#state{
                        subs = maps:put(Sid, Subject, State0#state.subs),
                        next_sid = Sid + 1
                    },
                    case State#state.handshake of
                        ready -> send_sub(State, Subject, Sid);
                        pending -> ok
                    end,
                    {reply, {ok, Sid}, State}
            end
    end;
handle_call(_Req, _From, State) ->
    {reply, {error, unknown}, State}.

handle_cast({publish, Subject, Payload, Headers}, State) ->
    case valid_subject(Subject) andalso is_binary(Payload)
         andalso byte_size(Payload) =< ?MAX_PAYLOAD_BYTES
         andalso is_list(Headers) of
        true -> send_pub(State, Subject, Payload, Headers);
        false -> ok
    end,
    {noreply, State};
handle_cast({unsubscribe, Sid}, State) ->
    send_unsub(State, Sid),
    {noreply, State#state{subs = maps:remove(Sid, State#state.subs)}};
handle_cast(_Msg, State) ->
    {noreply, State}.

handle_info(connect, State) ->
    do_connect(State);
handle_info({tcp, Sock, Data}, #state{sock = Sock} = State) ->
    Combined = <<(State#state.buf)/binary, Data/binary>>,
    case byte_size(Combined) > ?MAX_BUF_BYTES of
        true ->
            error_logger:warning_msg("nats: read buffer overflow, reconnecting~n"),
            close_sock(State),
            {noreply, schedule_reconnect(State#state{
                sock = undefined, buf = <<>>, handshake = pending
            })};
        false ->
            State1 = State#state{buf = Combined},
            State2 = process_buffer(State1),
            {noreply, State2}
    end;
handle_info({tcp_closed, Sock}, #state{sock = Sock} = State) ->
    error_logger:warning_msg("nats: tcp_closed, reconnecting~n"),
    {noreply, schedule_reconnect(State#state{
        sock = undefined, buf = <<>>, handshake = pending
    })};
handle_info({tcp_error, Sock, Reason}, #state{sock = Sock} = State) ->
    error_logger:warning_msg("nats: tcp_error ~p, reconnecting~n", [Reason]),
    {noreply, schedule_reconnect(State#state{
        sock = undefined, buf = <<>>, handshake = pending
    })};
handle_info(ping, State) ->
    case {State#state.sock, State#state.handshake} of
        {Sock, ready} when Sock =/= undefined ->
            safe_send(Sock, <<"PING\r\n">>);
        _ ->
            ok
    end,
    {noreply, schedule_ping(State)};
handle_info({'DOWN', Mon, process, Pid, _Reason},
            #state{notify = Pid, notify_mon = Mon} = State) ->
    {stop, notify_down, State};
handle_info(_Msg, State) ->
    {noreply, State}.

terminate(_Reason, #state{sock = undefined}) -> ok;
terminate(_Reason, #state{sock = Sock}) ->
    safe_close(Sock),
    ok.

code_change(_, S, _) -> {ok, S}.

%%% ---------------------------------------------------------------------
%%% Connection lifecycle
%%% ---------------------------------------------------------------------

do_connect(State) ->
    Opts = [binary, {packet, raw}, {active, true}, {keepalive, true},
            {nodelay, true}],
    case gen_tcp:connect(State#state.host, State#state.port, Opts, 5000) of
        {ok, Sock} ->
            error_logger:info_msg("nats: connected to ~s:~p~n",
                                 [State#state.host, State#state.port]),
            %% Server sends INFO immediately; replay SUBs only after CONNECT.
            {noreply, State#state{
                sock = Sock,
                backoff_ms = ?RECONNECT_MIN_MS,
                handshake = pending,
                buf = <<>>
            }};
        {error, Reason} ->
            error_logger:warning_msg("nats: connect failed ~p, retrying~n",
                                    [Reason]),
            {noreply, schedule_reconnect(State#state{handshake = pending})}
    end.

schedule_reconnect(State) ->
    close_sock(State),
    Delay = State#state.backoff_ms,
    Jitter = rand:uniform(Delay div 2 + 1),
    erlang:send_after(Delay + Jitter, self(), connect),
    NextBackoff = min(Delay * 2, ?RECONNECT_MAX_MS),
    State#state{backoff_ms = NextBackoff, sock = undefined, buf = <<>>,
                handshake = pending}.

close_sock(#state{sock = undefined}) -> ok;
close_sock(#state{sock = Sock}) -> safe_close(Sock).

schedule_ping(State) ->
    case State#state.ping_ref of
        undefined -> ok;
        ExistingRef -> erlang:cancel_timer(ExistingRef)
    end,
    NewRef = erlang:send_after(?PING_INTERVAL_MS, self(), ping),
    State#state{ping_ref = NewRef}.

replay_subs(State) ->
    maps:fold(
      fun(Sid, Subject, _) -> send_sub(State, Subject, Sid), ok end,
      ok, State#state.subs).

%%% ---------------------------------------------------------------------
%%% Wire send helpers
%%% ---------------------------------------------------------------------

send_sub(#state{sock = undefined}, _, _) -> ok;
send_sub(#state{handshake = pending}, _, _) -> ok;
send_sub(#state{sock = Sock}, Subject, Sid) ->
    case valid_subject(Subject) of
        false -> ok;
        true ->
            Cmd = <<"SUB ", Subject/binary, " ",
                    (integer_to_binary(Sid))/binary, "\r\n">>,
            safe_send(Sock, Cmd)
    end.

send_unsub(#state{sock = undefined}, _) -> ok;
send_unsub(#state{handshake = pending}, _) -> ok;
send_unsub(#state{sock = Sock}, Sid) when is_integer(Sid) ->
    Cmd = <<"UNSUB ", (integer_to_binary(Sid))/binary, "\r\n">>,
    safe_send(Sock, Cmd);
send_unsub(_, _) -> ok.

send_pub(#state{sock = undefined}, _, _, _) -> ok;
send_pub(#state{handshake = pending}, _, _, _) -> ok;
send_pub(#state{sock = Sock}, Subject, Payload, []) ->
    Size = byte_size(Payload),
    Cmd = <<"PUB ", Subject/binary, " ", (integer_to_binary(Size))/binary,
            "\r\n", Payload/binary, "\r\n">>,
    safe_send(Sock, Cmd);
send_pub(#state{sock = Sock}, Subject, Payload, Headers) ->
    HdrBin = encode_headers(Headers),
    HdrSize = byte_size(HdrBin),
    TotalSize = HdrSize + byte_size(Payload),
    Cmd = <<"HPUB ", Subject/binary, " ",
            (integer_to_binary(HdrSize))/binary, " ",
            (integer_to_binary(TotalSize))/binary, "\r\n",
            HdrBin/binary, Payload/binary, "\r\n">>,
    safe_send(Sock, Cmd).

encode_headers(Headers) ->
    iolist_to_binary([
        <<"NATS/1.0\r\n">>,
        [encode_header_line(K, V) || {K, V} <- Headers],
        <<"\r\n">>
    ]).

encode_header_line(K, V) ->
    KK = to_bin(K),
    VV = to_bin(V),
    case valid_header_token(KK) andalso valid_header_token(VV) of
        true -> [KK, <<": ">>, VV, <<"\r\n">>];
        false -> []
    end.

valid_header_token(Bin) when is_binary(Bin) ->
    binary:match(Bin, [<<"\r">>, <<"\n">>, <<0>>]) =:= nomatch;
valid_header_token(_) ->
    false.

to_bin(B) when is_binary(B) -> B;
to_bin(L) when is_list(L) -> unicode:characters_to_binary(L);
to_bin(A) when is_atom(A) -> atom_to_binary(A, utf8).

%%% ---------------------------------------------------------------------
%%% Incoming protocol parser
%%% ---------------------------------------------------------------------

process_buffer(State) ->
    case process_one(State#state.buf, State) of
        {more, NewBuf, State1} -> State1#state{buf = NewBuf};
        {done, NewBuf, State1} -> process_buffer(State1#state{buf = NewBuf})
    end.

process_one(Buf, State) ->
    case binary:split(Buf, <<"\r\n">>) of
        [_] -> {more, Buf, State};
        [Line, Rest] ->
            handle_line(Line, Rest, State)
    end.

handle_line(<<"INFO ", _Json/binary>>, Rest, State) ->
    Connect = build_connect_json(State),
    case State#state.sock of
        undefined -> ok;
        Sock ->
            safe_send(Sock, <<"CONNECT ", Connect/binary, "\r\n">>)
    end,
    State1 = State#state{handshake = ready},
    replay_subs(State1),
    {done, Rest, State1};
handle_line(<<"PING">>, Rest, State) ->
    case State#state.sock of
        undefined -> ok;
        Sock -> safe_send(Sock, <<"PONG\r\n">>)
    end,
    {done, Rest, State};
handle_line(<<"PONG">>, Rest, State) ->
    {done, Rest, State};
handle_line(<<"+OK">>, Rest, State) ->
    {done, Rest, State};
handle_line(<<"-ERR ", Reason/binary>>, Rest, State) ->
    error_logger:warning_msg("nats: server error ~s~n", [Reason]),
    {done, Rest, State};
handle_line(<<"MSG ", Args/binary>> = Line, Rest, State) ->
    Parts = binary:split(Args, <<" ">>, [global]),
    case Parts of
        [Subject, Sid, BytesBin] ->
            handle_msg(Line, Subject, Sid, BytesBin, Rest, [], State);
        [Subject, Sid, _Reply, BytesBin] ->
            handle_msg(Line, Subject, Sid, BytesBin, Rest, [], State);
        _ ->
            {done, Rest, State}
    end;
handle_line(<<"HMSG ", Args/binary>> = Line, Rest, State) ->
    Parts = binary:split(Args, <<" ">>, [global]),
    case Parts of
        [Subject, Sid, HBytesBin, TBytesBin] ->
            handle_hmsg(Line, Subject, Sid, HBytesBin, TBytesBin, Rest, State);
        [Subject, Sid, _Reply, HBytesBin, TBytesBin] ->
            handle_hmsg(Line, Subject, Sid, HBytesBin, TBytesBin, Rest, State);
        _ ->
            {done, Rest, State}
    end;
handle_line(_Other, Rest, State) ->
    {done, Rest, State}.

handle_msg(Line, Subject, _Sid, BytesBin, Rest, Headers, State) ->
    case parse_int(BytesBin) of
        {ok, Bytes} when Bytes >= 0, Bytes =< ?MAX_PAYLOAD_BYTES ->
            case Rest of
                <<Payload:Bytes/binary, "\r\n", Tail/binary>> ->
                    notify_msg(State, Subject, Payload, Headers),
                    {done, Tail, State};
                _ ->
                    {more, <<Line/binary, "\r\n", Rest/binary>>, State}
            end;
        _ ->
            {done, Rest, State}
    end.

handle_hmsg(Line, Subject, _Sid, HBytesBin, TBytesBin, Rest, State) ->
    case {parse_int(HBytesBin), parse_int(TBytesBin)} of
        {{ok, HBytes}, {ok, TBytes}}
          when is_integer(HBytes), is_integer(TBytes),
               HBytes >= 0, TBytes >= HBytes,
               TBytes =< ?MAX_PAYLOAD_BYTES ->
            BodyLen = TBytes - HBytes,
            case Rest of
                <<HdrBin:HBytes/binary, Payload:BodyLen/binary, "\r\n", Tail/binary>> ->
                    Headers = parse_headers(HdrBin),
                    notify_msg(State, Subject, Payload, Headers),
                    {done, Tail, State};
                _ ->
                    {more, <<Line/binary, "\r\n", Rest/binary>>, State}
            end;
        _ ->
            {done, Rest, State}
    end.

notify_msg(#state{notify = Notify}, Subject, Payload, Headers) ->
    case valid_subject(Subject) of
        false -> ok;
        true -> Notify ! {nats_msg, Subject, Payload, Headers}
    end.

parse_headers(<<"NATS/1.0\r\n", Rest/binary>>) -> parse_header_lines(Rest, []);
parse_headers(<<"NATS/1.0\n", Rest/binary>>) -> parse_header_lines(Rest, []);
parse_headers(_) -> [].

parse_header_lines(<<"\r\n", _/binary>>, Acc) -> lists:reverse(Acc);
parse_header_lines(<<>>, Acc) -> lists:reverse(Acc);
parse_header_lines(Bin, Acc) ->
    case binary:split(Bin, <<"\r\n">>) of
        [<<>>, _] -> lists:reverse(Acc);
        [Line, Rest] ->
            case split_header_line(Line) of
                {K, V} -> parse_header_lines(Rest, [{K, V} | Acc]);
                skip -> parse_header_lines(Rest, Acc)
            end;
        [_] -> lists:reverse(Acc)
    end.

split_header_line(Line) ->
    case binary:split(Line, <<": ">>) of
        [K, V] -> {K, V};
        [_] ->
            case binary:split(Line, <<":">>) of
                [K2, V2] -> {K2, trim_leading_space(V2)};
                _ -> skip
            end
    end.

trim_leading_space(<<" ", Rest/binary>>) -> trim_leading_space(Rest);
trim_leading_space(Bin) -> Bin.

first_header_ci([], _) -> undefined;
first_header_ci([{K, V} | Rest], Want) ->
    case string:lowercase(to_bin(K)) of
        Want -> to_bin(V);
        _ -> first_header_ci(Rest, Want)
    end.

%%% ---------------------------------------------------------------------
%%% URL parsing and CONNECT json
%%% ---------------------------------------------------------------------

parse_url(UrlBin) when is_binary(UrlBin) ->
    parse_url(binary_to_list(UrlBin));
parse_url(UrlStr) when is_list(UrlStr) ->
    try parse_url_str(UrlStr)
    catch
        _:_ -> {error, bad_url}
    end;
parse_url(_) ->
    {error, bad_url}.

parse_url_str(UrlStr) ->
    case lists:prefix("nats://", UrlStr) of
        true ->
            Body = lists:nthtail(7, UrlStr),
            {Auth, HostPart} =
                case string:split(Body, "@") of
                    [HostOnly] -> {undefined, HostOnly};
                    [A, H] -> {A, H};
                    _ -> throw(bad_auth)
                end,
            {User, Pass} =
                case Auth of
                    undefined -> {undefined, undefined};
                    "" -> {undefined, undefined};
                    _ ->
                        case string:split(Auth, ":") of
                            [U] -> {U, undefined};
                            [U, P] -> {U, P};
                            _ -> throw(bad_auth)
                        end
                end,
            {Host, Port} =
                case string:split(HostPart, ":") of
                    [HostOnly2] -> {HostOnly2, 4222};
                    [HA, PStr] -> {HA, list_to_integer(PStr)};
                    _ -> throw(bad_host)
                end,
            case Host of
                "" -> {error, bad_host};
                _ -> {ok, Host, Port, User, Pass}
            end;
        false ->
            {error, bad_scheme}
    end.

build_connect_json(#state{user = undefined}) ->
    <<"{\"verbose\":false,\"pedantic\":false,\"tls_required\":false,"
      "\"name\":\"dd_nats\",\"lang\":\"erlang\","
      "\"version\":\"1.0\",\"headers\":true,\"no_responders\":true}">>;
build_connect_json(#state{user = User, pass = undefined}) ->
    Base = build_connect_json(#state{user = undefined}),
    UserBin = json_escape(User),
    OpenObj = binary:part(Base, 0, byte_size(Base) - 1),
    <<OpenObj/binary, ",\"user\":\"", UserBin/binary, "\"}">>;
build_connect_json(#state{user = User, pass = Pass}) ->
    Base = build_connect_json(#state{user = undefined}),
    UserBin = json_escape(User),
    PassBin = json_escape(Pass),
    OpenObj = binary:part(Base, 0, byte_size(Base) - 1),
    <<OpenObj/binary, ",\"user\":\"", UserBin/binary,
      "\",\"pass\":\"", PassBin/binary, "\"}">>.

escape_json_bin(Bin) ->
    << <<(escape_json_byte(C))/binary>> || <<C>> <= Bin >>.

escape_json_byte($\\) -> <<"\\\\">>;
escape_json_byte($") -> <<"\\\"">>;
escape_json_byte($\n) -> <<"\\n">>;
escape_json_byte($\r) -> <<"\\r">>;
escape_json_byte($\t) -> <<"\\t">>;
escape_json_byte(C) when C < 32 -> <<" ">>;
escape_json_byte(C) -> <<C>>.

safe_send(Sock, IoData) ->
    try gen_tcp:send(Sock, IoData) of
        ok -> ok;
        {error, _} -> ok
    catch
        _:_ -> ok
    end.

safe_close(Sock) ->
    try gen_tcp:close(Sock) of
        ok -> ok;
        {error, _} -> ok
    catch
        _:_ -> ok
    end.

parse_int(Bin) when is_binary(Bin) ->
    try binary_to_integer(Bin) of
        N when is_integer(N) -> {ok, N}
    catch
        error:badarg -> error
    end;
parse_int(_) ->
    error.
