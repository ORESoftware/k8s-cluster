-module(lambda_child_runner).

-export([invoke/5, metrics/0, destroy/1]).

-define(SERVER, lambda_child_runner_manager).
-define(WORKERS, lambda_child_runner_workers).
-define(METRICS, lambda_child_runner_metrics).

invoke(Command0, ReuseKey0, Payload0, IdleMs0, TimeoutMs0) ->
    ensure_tables(),
    Command = to_binary(Command0),
    ReuseKey = to_binary(ReuseKey0),
    RequestPayload0 = normalize_json_payload(to_binary(Payload0)),
    RequestPayload = case RequestPayload0 of
        <<>> -> <<"null">>;
        _ -> RequestPayload0
    end,
    IdleMs = max_int(IdleMs0, 1000),
    TimeoutMs = max_int(TimeoutMs0, 1000),
    reap_idle(now_ms()),
    case load_function_definition(ReuseKey) of
        {ok, DefinitionJson} ->
            Payload = invocation_payload(ReuseKey, DefinitionJson, RequestPayload),
            bump(invocations_total, 1),
            invoke_worker(Command, ReuseKey, Payload, IdleMs, TimeoutMs);
        {error, Reason} ->
            {error, Reason}
    end.

invoke_worker(Command, ReuseKey, Payload, IdleMs, TimeoutMs) ->
    case ensure_worker(Command, ReuseKey, IdleMs) of
        {ok, Pid} ->
            Ref = make_ref(),
            Monitor = erlang:monitor(process, Pid),
            Pid ! {invoke, self(), Ref, Payload},
            receive
                {Ref, {ok, Data}} ->
                    erlang:demonitor(Monitor, [flush]),
                    byte_bump(child_stdio_bytes_total, Data),
                    io:format(
                        "lambda_child_stdio reuse_key=~s bytes=~p~n",
                        [safe_label(ReuseKey), byte_size(Data)]
                    ),
                    update_last_used(ReuseKey),
                    {ok, Data};
                {Ref, {exit_status, Status}} ->
                    erlang:demonitor(Monitor, [flush]),
                    delete_worker(ReuseKey),
                    bump(child_exits_total, 1),
                    {error, iolist_to_binary(io_lib:format("child exited with status ~p", [Status]))};
                {Ref, {error, Reason}} ->
                    erlang:demonitor(Monitor, [flush]),
                    delete_worker(ReuseKey),
                    {error, Reason};
                {'DOWN', Monitor, process, Pid, Reason} ->
                    delete_worker(ReuseKey),
                    bump(child_exits_total, 1),
                    {error, iolist_to_binary(io_lib:format("child worker exited: ~p", [Reason]))}
            after TimeoutMs ->
                Pid ! stop,
                erlang:demonitor(Monitor, [flush]),
                delete_worker(ReuseKey),
                bump(invocation_timeouts_total, 1),
                {error, <<"lambda child process timed out">>}
            end;
        {error, Reason} ->
            {error, Reason}
    end.

metrics() ->
    ensure_tables(),
    ActiveWorkers = ets:info(?WORKERS, size),
    iolist_to_binary([
        "# HELP dd_lambda_runner_invocations_total Lambda invocations handled by the Gleam runner.\n",
        "# TYPE dd_lambda_runner_invocations_total counter\n",
        metric_line("dd_lambda_runner_invocations_total", get_metric(invocations_total)),
        "# HELP dd_lambda_runner_child_spawns_total Child processes spawned by the Gleam runner.\n",
        "# TYPE dd_lambda_runner_child_spawns_total counter\n",
        metric_line("dd_lambda_runner_child_spawns_total", get_metric(child_spawns_total)),
        "# HELP dd_lambda_runner_child_reuses_total Child process reuse hits.\n",
        "# TYPE dd_lambda_runner_child_reuses_total counter\n",
        metric_line("dd_lambda_runner_child_reuses_total", get_metric(child_reuses_total)),
        "# HELP dd_lambda_runner_child_destroys_total Child processes destroyed by idle reaping or command changes.\n",
        "# TYPE dd_lambda_runner_child_destroys_total counter\n",
        metric_line("dd_lambda_runner_child_destroys_total", get_metric(child_destroys_total)),
        "# HELP dd_lambda_runner_child_exits_total Child processes that exited during invocation.\n",
        "# TYPE dd_lambda_runner_child_exits_total counter\n",
        metric_line("dd_lambda_runner_child_exits_total", get_metric(child_exits_total)),
        "# HELP dd_lambda_runner_invocation_timeouts_total Lambda child invocations that timed out.\n",
        "# TYPE dd_lambda_runner_invocation_timeouts_total counter\n",
        metric_line("dd_lambda_runner_invocation_timeouts_total", get_metric(invocation_timeouts_total)),
        "# HELP dd_lambda_runner_child_stdio_bytes_total Bytes read from child process stdio.\n",
        "# TYPE dd_lambda_runner_child_stdio_bytes_total counter\n",
        metric_line("dd_lambda_runner_child_stdio_bytes_total", get_metric(child_stdio_bytes_total)),
        "# HELP dd_lambda_runner_active_workers Active reusable child processes.\n",
        "# TYPE dd_lambda_runner_active_workers gauge\n",
        metric_line("dd_lambda_runner_active_workers", ActiveWorkers)
    ]).

destroy(ReuseKey0) ->
    ensure_tables(),
    ReuseKey = to_binary(ReuseKey0),
    case ets:lookup(?WORKERS, ReuseKey) of
        [{ReuseKey, Worker}] ->
            close_worker(maps:get(pid, Worker)),
            delete_worker(ReuseKey),
            bump(child_destroys_total, 1),
            {ok, <<"destroyed">>};
        [] ->
            {ok, <<"not-found">>}
    end.

load_function_definition(Identifier) ->
    case identifier_kind(Identifier) of
        invalid ->
            {error, <<"valid lambda function UUID or slug is required">>};
        Kind ->
            case database_url() of
                {ok, DatabaseUrl} ->
                    load_function_definition(Kind, Identifier, DatabaseUrl);
                {error, Reason} ->
                    {error, Reason}
            end
    end.

load_function_definition(Kind, Identifier, DatabaseUrl) ->
    case os:find_executable("psql") of
        false ->
            {error, <<"psql executable not found">>};
        Psql ->
            Sql = lambda_definition_sql(Kind, Identifier),
            case run_psql(Psql, DatabaseUrl, Sql) of
                {ok, <<>>} ->
                    {error, iolist_to_binary(["lambda function not found: ", Identifier])};
                {ok, DefinitionJson} ->
                    {ok, DefinitionJson};
                {error, Reason} ->
                    {error, Reason}
            end
    end.

lambda_definition_sql(Kind, Identifier) ->
    iolist_to_binary([
        "select jsonb_build_object(",
        "'id', id::text,",
        "'slug', slug,",
        "'functionBody', function_body,",
        "'runtime', runtime,",
        "'entryCommand', entry_command,",
        "'reuseKey', reuse_key,",
        "'idleTimeoutSeconds', idle_timeout_seconds,",
        "'maxRunMs', max_run_ms,",
        "'status', status,",
        "'labels', labels,",
        "'metaData', meta_data",
        ")::text ",
        "from lambda_functions ",
        "where ",
        identifier_where_clause(Kind, Identifier),
        " ",
        "and is_soft_deleted = false ",
        "limit 1"
    ]).

identifier_where_clause(uuid, Identifier) ->
    ["id = '", Identifier, "'::uuid"];
identifier_where_clause(slug, Identifier) ->
    ["slug = '", Identifier, "'"].

run_psql(Psql, DatabaseUrl, Sql) ->
    Port = open_port({spawn_executable, Psql}, [
        binary,
        exit_status,
        stderr_to_stdout,
        use_stdio,
        {args, [
            DatabaseUrl,
            "-X",
            "-q",
            "-At",
            "-v",
            "ON_ERROR_STOP=1",
            "-c",
            binary_to_list(Sql)
        ]}
    ]),
    collect_port(Port, [], 0, 5000).

collect_port(Port, Chunks, Size, TimeoutMs) ->
    receive
        {Port, {data, Data}} ->
            NewSize = Size + byte_size(Data),
            case NewSize > 1048576 of
                true ->
                    close_port(Port),
                    {error, <<"lambda definition query exceeded byte limit">>};
                false ->
                    collect_port(Port, [Data | Chunks], NewSize, TimeoutMs)
            end;
        {Port, {exit_status, 0}} ->
            {ok, normalize_json_payload(iolist_to_binary(lists:reverse(Chunks)))};
        {Port, {exit_status, Status}} ->
            Output = normalize_json_payload(iolist_to_binary(lists:reverse(Chunks))),
            {error, iolist_to_binary(io_lib:format("psql exited with status ~p: ~s", [Status, Output]))}
    after TimeoutMs ->
        close_port(Port),
        {error, <<"lambda definition query timed out">>}
    end.

database_url() ->
    case os:getenv("LAMBDA_DATABASE_URL") of
        false -> {error, <<"LAMBDA_DATABASE_URL is required">>};
        "" -> {error, <<"LAMBDA_DATABASE_URL is required">>};
        Value -> {ok, Value}
    end.

identifier_kind(Identifier) ->
    case re:run(Identifier, "^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$", [{capture, none}]) of
        match ->
            uuid;
        nomatch ->
            case re:run(Identifier, "^[a-z0-9][a-z0-9-]{1,118}[a-z0-9]$", [{capture, none}]) of
                match -> slug;
                nomatch -> invalid
            end
    end.

invocation_payload(Slug, DefinitionJson, RequestJson) ->
    iolist_to_binary([
        "{\"slug\":\"",
        json_escape(Slug),
        "\",\"definition\":",
        DefinitionJson,
        ",\"request\":",
        RequestJson,
        "}"
    ]).

ensure_tables() ->
    ensure_manager(),
    wait_for_tables(500).

ensure_manager() ->
    case whereis(?SERVER) of
        undefined ->
            Pid = spawn(fun manager_bootstrap/0),
            case catch register(?SERVER, Pid) of
                true ->
                    Pid ! start,
                    ok;
                {'EXIT', _Reason} ->
                    Pid ! stop,
                    ok
            end;
        _Pid ->
            ok
    end.

manager_bootstrap() ->
    receive
        start ->
            ensure_table(?WORKERS),
            ensure_table(?METRICS),
            manager_loop();
        stop ->
            ok
    after 5000 ->
        ok
    end.

manager_loop() ->
    receive
        {call, From, Ref, {ensure_worker, Command, ReuseKey, IdleMs}} ->
            From ! {Ref, ensure_worker_in_manager(Command, ReuseKey, IdleMs)},
            manager_loop();
        stop -> ok;
        _Other -> manager_loop()
    end.

manager_call(Message) ->
    case whereis(?SERVER) of
        undefined ->
            {error, <<"lambda runner manager unavailable">>};
        Pid ->
            Ref = make_ref(),
            Pid ! {call, self(), Ref, Message},
            receive
                {Ref, Result} -> Result
            after 5000 ->
                {error, <<"lambda runner manager timed out">>}
            end
    end.

wait_for_tables(Attempts) when Attempts > 0 ->
    case {ets:info(?WORKERS), ets:info(?METRICS)} of
        {undefined, _} ->
            timer:sleep(10),
            wait_for_tables(Attempts - 1);
        {_, undefined} ->
            timer:sleep(10),
            wait_for_tables(Attempts - 1);
        _ ->
            ok
    end;
wait_for_tables(_Attempts) ->
    erlang:error(lambda_child_runner_manager_unavailable).

ensure_table(Name) ->
    case ets:info(Name) of
        undefined ->
            ets:new(Name, [named_table, public, set]),
            ok;
        _ ->
            ok
    end.

ensure_worker(Command, ReuseKey, IdleMs) ->
    manager_call({ensure_worker, Command, ReuseKey, IdleMs}).

ensure_worker_in_manager(Command, ReuseKey, IdleMs) ->
    case ets:lookup(?WORKERS, ReuseKey) of
        [{ReuseKey, Worker}] ->
            ExistingCommand = maps:get(command, Worker),
            Pid = maps:get(pid, Worker),
            case ExistingCommand =:= Command andalso worker_alive(Pid) of
                true ->
                    bump(child_reuses_total, 1),
                    {ok, Pid};
                false ->
                    close_worker(Pid),
                    delete_worker(ReuseKey),
                    spawn_worker(Command, ReuseKey, IdleMs)
            end;
        [] ->
            spawn_worker(Command, ReuseKey, IdleMs)
    end.

spawn_worker(Command, ReuseKey, IdleMs) ->
    Parent = self(),
    Pid = spawn(fun() -> worker_start(Parent, Command) end),
    receive
        {Pid, started} ->
            ets:insert(?WORKERS, {
                ReuseKey,
                #{
                    command => Command,
                    pid => Pid,
                    idle_ms => IdleMs,
                    last_used_ms => now_ms()
                }
            }),
            bump(child_spawns_total, 1),
            {ok, Pid};
        {Pid, failed, Reason} ->
            {error, Reason}
    after 5000 ->
        Pid ! stop,
        {error, <<"timed out starting lambda child process">>}
    end.

worker_start(Parent, Command) ->
    try open_port({spawn, binary_to_list(Command)}, [binary, exit_status, use_stdio]) of
        Port ->
            Parent ! {self(), started},
            worker_loop(Port)
    catch
        Class:Reason ->
            Parent ! {
                self(),
                failed,
                iolist_to_binary(io_lib:format("failed to spawn child process: ~p:~p", [Class, Reason]))
            }
    end.

worker_loop(Port) ->
    receive
        {invoke, From, Ref, Payload} ->
            port_command(Port, [Payload, <<"\n">>]),
            worker_receive_result(Port, From, Ref);
        {Port, {exit_status, _Status}} ->
            ok;
        stop ->
            close_port(Port)
    end.

worker_receive_result(Port, From, Ref) ->
    receive
        {Port, {data, Data}} ->
            From ! {Ref, {ok, Data}},
            worker_loop(Port);
        {Port, {exit_status, Status}} ->
            From ! {Ref, {exit_status, Status}};
        stop ->
            close_port(Port),
            From ! {Ref, {error, <<"lambda child worker stopped">>}}
    end.

update_last_used(ReuseKey) ->
    case ets:lookup(?WORKERS, ReuseKey) of
        [{ReuseKey, Worker}] ->
            ets:insert(?WORKERS, {ReuseKey, Worker#{last_used_ms => now_ms()}});
        [] ->
            ok
    end.

reap_idle(NowMs) ->
    lists:foreach(
        fun({ReuseKey, Worker}) ->
            LastUsed = maps:get(last_used_ms, Worker),
            IdleMs = maps:get(idle_ms, Worker),
            case NowMs - LastUsed > IdleMs of
                true ->
                    close_worker(maps:get(pid, Worker)),
                    delete_worker(ReuseKey),
                    bump(child_destroys_total, 1);
                false ->
                    ok
            end
        end,
        ets:tab2list(?WORKERS)
    ).

delete_worker(ReuseKey) ->
    ets:delete(?WORKERS, ReuseKey).

close_port(Port) ->
    case port_alive(Port) of
        true -> catch port_close(Port);
        false -> ok
    end.

port_alive(Port) ->
    is_port(Port) andalso erlang:port_info(Port) =/= undefined.

close_worker(Pid) ->
    case worker_alive(Pid) of
        true -> Pid ! stop;
        false -> ok
    end.

worker_alive(Pid) ->
    is_pid(Pid) andalso erlang:is_process_alive(Pid).

metric_line(Name, Value) ->
    io_lib:format("~s{service=\"dd-gleam-lambda-runner\"} ~p~n", [Name, Value]).

get_metric(Name) ->
    case ets:lookup(?METRICS, Name) of
        [{Name, Value}] -> Value;
        [] -> 0
    end.

bump(Name, Amount) ->
    ets:update_counter(?METRICS, Name, Amount, {Name, 0}).

byte_bump(Name, Data) ->
    bump(Name, byte_size(Data)).

now_ms() ->
    erlang:system_time(millisecond).

max_int(Value, Min) when is_integer(Value), Value >= Min ->
    Value;
max_int(_Value, Min) ->
    Min.

to_binary(Value) when is_binary(Value) ->
    Value;
to_binary(Value) when is_list(Value) ->
    unicode:characters_to_binary(Value);
to_binary(Value) ->
    unicode:characters_to_binary(io_lib:format("~p", [Value])).

normalize_json_payload(Value) ->
    unicode:characters_to_binary(string:trim(binary_to_list(to_binary(Value)))).

json_escape(Value0) ->
    Value = to_binary(Value0),
    EscapedSlash = binary:replace(Value, <<"\\">>, <<"\\\\">>, [global]),
    binary:replace(EscapedSlash, <<"\"">>, <<"\\\"">>, [global]).

safe_label(Value) ->
    binary_to_list(binary:replace(Value, <<"\"">>, <<"">>, [global])).
