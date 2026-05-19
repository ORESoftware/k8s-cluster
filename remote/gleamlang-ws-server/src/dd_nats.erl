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
%%%   - SUB <subject> <sid>\r\n
%%%   - PUB <subject> <bytes>\r\npayload\r\n
%%%   - HPUB <subject> <hdr_bytes> <total_bytes>\r\nhdr+payload\r\n  (NATS 2.0 hdr)
%%%   - MSG / HMSG dispatch to a per-subscription forwarder pid.
%%%   - PING / PONG keepalive (server-driven; we also send PING on idle).
%%%   - Auto reconnect with jittered exponential backoff (1s → 30s).
%%%
%%% What we deliberately skip:
%%%   - Authentication beyond optional user/pass in the URL.
%%%   - Cluster info discovery (we use a single URL).
%%%   - JetStream and KV stores.
%%%   - TLS (add later if needed).
%%%
%%% Public API (called from Gleam via FFI):
%%%   start_link(Url, Notify)         — boot; Notify is a pid that
%%%                                     receives {nats_msg, Subject,
%%%                                     Payload, Headers} dispatches.
%%%   publish(Pid, Subject, Payload, Headers)
%%%   subscribe(Pid, Subject)         — returns {ok, Sid}
%%%   unsubscribe(Pid, Sid)
%%%   self_node_binary/0              — for source-node headers.
%%% =====================================================================
-module(dd_nats).

-behaviour(gen_server).

-export([
    start_link/2,
    publish/4,
    subscribe/2,
    unsubscribe/2,
    stop/1
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

-record(state, {
    url :: binary(),
    host :: string(),
    port :: pos_integer(),
    user :: undefined | string(),
    pass :: undefined | string(),
    sock = undefined :: undefined | gen_tcp:socket(),
    buf = <<>> :: binary(),
    notify :: pid(),
    %% Pending subscriptions to (re-)send on (re)connect. sid → subject.
    subs = #{} :: #{integer() => binary()},
    next_sid = 1 :: pos_integer(),
    backoff_ms = ?RECONNECT_MIN_MS :: pos_integer(),
    ping_ref = undefined :: undefined | reference()
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

%%% ---------------------------------------------------------------------
%%% gen_server callbacks
%%% ---------------------------------------------------------------------

init([Url, Notify]) ->
    process_flag(trap_exit, true),
    case parse_url(Url) of
        {ok, Host, Port, User, Pass} ->
            self() ! connect,
            {ok, schedule_ping(#state{
                url = Url,
                host = Host,
                port = Port,
                user = User,
                pass = Pass,
                notify = Notify
            })};
        {error, Reason} ->
            {stop, {bad_nats_url, Reason}}
    end.

handle_call({subscribe, Subject}, _From, State0) ->
    Sid = State0#state.next_sid,
    State = State0#state{
        subs = maps:put(Sid, Subject, State0#state.subs),
        next_sid = Sid + 1
    },
    send_sub(State, Subject, Sid),
    {reply, {ok, Sid}, State};
handle_call(_Req, _From, State) ->
    {reply, {error, unknown}, State}.

handle_cast({publish, Subject, Payload, Headers}, State) ->
    send_pub(State, Subject, Payload, Headers),
    {noreply, State};
handle_cast({unsubscribe, Sid}, State) ->
    send_unsub(State, Sid),
    {noreply, State#state{subs = maps:remove(Sid, State#state.subs)}};
handle_cast(_Msg, State) ->
    {noreply, State}.

handle_info(connect, State) ->
    do_connect(State);
handle_info({tcp, Sock, Data}, #state{sock = Sock} = State) ->
    State1 = State#state{buf = <<(State#state.buf)/binary, Data/binary>>},
    State2 = process_buffer(State1),
    {noreply, State2};
handle_info({tcp_closed, Sock}, #state{sock = Sock} = State) ->
    error_logger:warning_msg("nats: tcp_closed, reconnecting~n"),
    {noreply, schedule_reconnect(State#state{sock = undefined, buf = <<>>})};
handle_info({tcp_error, Sock, Reason}, #state{sock = Sock} = State) ->
    error_logger:warning_msg("nats: tcp_error ~p, reconnecting~n", [Reason]),
    {noreply, schedule_reconnect(State#state{sock = undefined, buf = <<>>})};
handle_info(ping, State) ->
    case State#state.sock of
        undefined -> ok;
        Sock -> gen_tcp:send(Sock, <<"PING\r\n">>)
    end,
    {noreply, schedule_ping(State)};
handle_info(_Msg, State) ->
    {noreply, State}.

terminate(_Reason, #state{sock = undefined}) -> ok;
terminate(_Reason, #state{sock = Sock}) ->
    catch gen_tcp:close(Sock),
    ok.

code_change(_, S, _) -> {ok, S}.

%%% ---------------------------------------------------------------------
%%% Connection lifecycle
%%% ---------------------------------------------------------------------

do_connect(State) ->
    Opts = [binary, {packet, raw}, {active, true}, {keepalive, true}],
    case gen_tcp:connect(State#state.host, State#state.port, Opts, 5000) of
        {ok, Sock} ->
            error_logger:info_msg("nats: connected to ~s:~p~n",
                                 [State#state.host, State#state.port]),
            %% Server sends INFO immediately; we'll reply on the next
            %% tcp event in process_buffer.
            State1 = State#state{sock = Sock, backoff_ms = ?RECONNECT_MIN_MS},
            %% Re-issue any in-flight subscriptions.
            maps:fold(
              fun(Sid, Subject, _) -> send_sub(State1, Subject, Sid), ok end,
              ok, State#state.subs),
            {noreply, State1};
        {error, Reason} ->
            error_logger:warning_msg("nats: connect failed ~p, retrying~n",
                                    [Reason]),
            {noreply, schedule_reconnect(State)}
    end.

schedule_reconnect(State) ->
    Delay = State#state.backoff_ms,
    Jitter = rand:uniform(Delay div 2 + 1),
    erlang:send_after(Delay + Jitter, self(), connect),
    NextBackoff = min(Delay * 2, ?RECONNECT_MAX_MS),
    State#state{backoff_ms = NextBackoff}.

schedule_ping(State) ->
    case State#state.ping_ref of
        undefined -> ok;
        ExistingRef -> erlang:cancel_timer(ExistingRef)
    end,
    NewRef = erlang:send_after(?PING_INTERVAL_MS, self(), ping),
    State#state{ping_ref = NewRef}.

%%% ---------------------------------------------------------------------
%%% Wire send helpers
%%% ---------------------------------------------------------------------

send_sub(#state{sock = undefined}, _, _) -> ok;
send_sub(#state{sock = Sock}, Subject, Sid) ->
    Cmd = <<"SUB ", Subject/binary, " ", (integer_to_binary(Sid))/binary, "\r\n">>,
    gen_tcp:send(Sock, Cmd).

send_unsub(#state{sock = undefined}, _) -> ok;
send_unsub(#state{sock = Sock}, Sid) ->
    Cmd = <<"UNSUB ", (integer_to_binary(Sid))/binary, "\r\n">>,
    gen_tcp:send(Sock, Cmd).

send_pub(#state{sock = undefined}, _, _, _) -> ok;
send_pub(#state{sock = Sock}, Subject, Payload, []) ->
    Size = byte_size(Payload),
    Cmd = <<"PUB ", Subject/binary, " ", (integer_to_binary(Size))/binary,
            "\r\n", Payload/binary, "\r\n">>,
    gen_tcp:send(Sock, Cmd);
send_pub(#state{sock = Sock}, Subject, Payload, Headers) ->
    HdrBin = encode_headers(Headers),
    HdrSize = byte_size(HdrBin),
    TotalSize = HdrSize + byte_size(Payload),
    Cmd = <<"HPUB ", Subject/binary, " ",
            (integer_to_binary(HdrSize))/binary, " ",
            (integer_to_binary(TotalSize))/binary, "\r\n",
            HdrBin/binary, Payload/binary, "\r\n">>,
    gen_tcp:send(Sock, Cmd).

%% NATS 2.0 header line wire format:
%%   NATS/1.0\r\n
%%   Key: Value\r\n
%%   Key: Value\r\n
%%   \r\n
encode_headers(Headers) ->
    iolist_to_binary([
        <<"NATS/1.0\r\n">>,
        [[K, <<": ">>, V, <<"\r\n">>] || {K, V} <- Headers],
        <<"\r\n">>
    ]).

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
    %% Reply with CONNECT (no auth or with user/pass).
    Connect = build_connect_json(State),
    case State#state.sock of
        undefined -> ok;
        Sock ->
            gen_tcp:send(Sock, <<"CONNECT ", Connect/binary, "\r\n">>)
    end,
    {done, Rest, State};
handle_line(<<"PING">>, Rest, State) ->
    case State#state.sock of
        undefined -> ok;
        Sock -> gen_tcp:send(Sock, <<"PONG\r\n">>)
    end,
    {done, Rest, State};
handle_line(<<"PONG">>, Rest, State) ->
    {done, Rest, State};
handle_line(<<"+OK">>, Rest, State) ->
    {done, Rest, State};
handle_line(<<"-ERR ", Reason/binary>>, Rest, State) ->
    error_logger:warning_msg("nats: server error ~s~n", [Reason]),
    {done, Rest, State};
handle_line(<<"MSG ", Args/binary>>, Rest, State) ->
    %% "subject sid [reply-to] bytes"
    Parts = binary:split(Args, <<" ">>, [global]),
    case Parts of
        [Subject, Sid, BytesBin] ->
            handle_msg(Subject, Sid, undefined, BytesBin, Rest, [], State);
        [Subject, Sid, Reply, BytesBin] ->
            handle_msg(Subject, Sid, Reply, BytesBin, Rest, [], State);
        _ ->
            {done, Rest, State}
    end;
handle_line(<<"HMSG ", Args/binary>>, Rest, State) ->
    %% "subject sid [reply-to] hdr_bytes total_bytes"
    Parts = binary:split(Args, <<" ">>, [global]),
    case Parts of
        [Subject, Sid, HBytesBin, TBytesBin] ->
            handle_hmsg(Subject, Sid, undefined, HBytesBin, TBytesBin, Rest, State);
        [Subject, Sid, Reply, HBytesBin, TBytesBin] ->
            handle_hmsg(Subject, Sid, Reply, HBytesBin, TBytesBin, Rest, State);
        _ ->
            {done, Rest, State}
    end;
handle_line(_Other, Rest, State) ->
    %% Unknown verb; skip.
    {done, Rest, State}.

handle_msg(Subject, _Sid, _Reply, BytesBin, Rest, Headers, State) ->
    Bytes = binary_to_integer(BytesBin),
    case Rest of
        <<Payload:Bytes/binary, "\r\n", Tail/binary>> ->
            State#state.notify ! {nats_msg, Subject, Payload, Headers},
            {done, Tail, State};
        _ ->
            {more, <<"MSG ", Subject/binary, " 0 ", BytesBin/binary, "\r\n", Rest/binary>>, State}
    end.

handle_hmsg(Subject, _Sid, _Reply, HBytesBin, TBytesBin, Rest, State) ->
    HBytes = binary_to_integer(HBytesBin),
    TBytes = binary_to_integer(TBytesBin),
    case Rest of
        <<HdrBin:HBytes/binary, Payload:(TBytes - HBytes)/binary, "\r\n", Tail/binary>> ->
            Headers = parse_headers(HdrBin),
            State#state.notify ! {nats_msg, Subject, Payload, Headers},
            {done, Tail, State};
        _ ->
            {more, <<"HMSG ", Subject/binary, " 0 ", HBytesBin/binary, " ",
                    TBytesBin/binary, "\r\n", Rest/binary>>, State}
    end.

parse_headers(<<"NATS/1.0\r\n", Rest/binary>>) -> parse_header_lines(Rest, []);
parse_headers(_) -> [].

parse_header_lines(<<"\r\n", _/binary>>, Acc) -> lists:reverse(Acc);
parse_header_lines(<<>>, Acc) -> lists:reverse(Acc);
parse_header_lines(Bin, Acc) ->
    case binary:split(Bin, <<"\r\n">>) of
        [<<>>, _] -> lists:reverse(Acc);
        [Line, Rest] ->
            case binary:split(Line, <<": ">>) of
                [K, V] -> parse_header_lines(Rest, [{K, V} | Acc]);
                _ -> parse_header_lines(Rest, Acc)
            end;
        [_] -> lists:reverse(Acc)
    end.

%%% ---------------------------------------------------------------------
%%% URL parsing and CONNECT json
%%% ---------------------------------------------------------------------

parse_url(UrlBin) ->
    UrlStr = binary_to_list(UrlBin),
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
                            [U, P] -> {U, P}
                        end
                end,
            {Host, Port} =
                case string:split(HostPart, ":") of
                    [HostOnly2] -> {HostOnly2, 4222};
                    [HA, PStr] -> {HA, list_to_integer(PStr)}
                end,
            {ok, Host, Port, User, Pass};
        false ->
            {error, bad_scheme}
    end.

build_connect_json(#state{user = undefined}) ->
    <<"{\"verbose\":false,\"pedantic\":false,\"tls_required\":false,"
      "\"name\":\"gleamlang_ws_server\",\"lang\":\"erlang\","
      "\"version\":\"1.0\",\"headers\":true,\"no_responders\":true}">>;
build_connect_json(#state{user = User, pass = undefined}) ->
    Base = build_connect_json(#state{user = undefined}),
    UserBin = list_to_binary(User),
    OpenObj = binary:part(Base, 0, byte_size(Base) - 1),
    <<OpenObj/binary, ",\"user\":\"", UserBin/binary, "\"}">>;
build_connect_json(#state{user = User, pass = Pass}) ->
    Base = build_connect_json(#state{user = undefined}),
    UserBin = list_to_binary(User),
    PassBin = list_to_binary(Pass),
    OpenObj = binary:part(Base, 0, byte_size(Base) - 1),
    <<OpenObj/binary, ",\"user\":\"", UserBin/binary,
      "\",\"pass\":\"", PassBin/binary, "\"}">>.
