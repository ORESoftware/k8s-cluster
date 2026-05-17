-module(lambda_child_runner).

-export([invoke/5, metrics/0, destroy/1]).

-define(SERVER, lambda_child_runner_manager).
-define(WORKERS, lambda_child_runner_workers).
-define(METRICS, lambda_child_runner_metrics).

invoke(Command0, Identifier0, Payload0, IdleMs0, TimeoutMs0) ->
    ensure_tables(),
    FallbackCommand = to_binary(Command0),
    Identifier = to_binary(Identifier0),
    RequestPayload0 = normalize_json_payload(to_binary(Payload0)),
    RequestPayload = case RequestPayload0 of
        <<>> -> <<"null">>;
        _ -> RequestPayload0
    end,
    reap_idle(now_ms()),
    case load_function_definition(Identifier) of
        {ok, DefinitionJson} ->
            case command_for_definition(FallbackCommand, DefinitionJson) of
                {ok, Command} ->
                    Runtime = runtime_from_definition(DefinitionJson),
                    Containerized = json_bool_field(DefinitionJson, <<"containerized">>, false),
                    case worker_key(Identifier, DefinitionJson, Runtime, Containerized) of
                        {ok, WorkerKey} ->
                            IdleMs = idle_ms_from_definition(DefinitionJson, IdleMs0),
                            TimeoutMs = timeout_ms_from_definition(DefinitionJson, TimeoutMs0),
                            Payload = invocation_payload(Identifier, DefinitionJson, RequestPayload),
                            bump(invocations_total, 1),
                            invoke_worker(Command, WorkerKey, Payload, IdleMs, TimeoutMs);
                        {error, Reason} ->
                            {error, Reason}
                    end;
                {error, Reason} ->
                    {error, Reason}
            end;
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
    SelectSql = 'gleam_lambda_runner@pg_contract':lambda_functions_select_sql(),
    iolist_to_binary([
        "select jsonb_build_object(",
        "'id', id,",
        "'slug', slug,",
        "'functionBody', function_body,",
        "'runtime', runtime,",
        "'entryCommand', entry_command,",
        "'reuseKey', reuse_key,",
        "'idleTimeoutSeconds', idle_timeout_seconds,",
        "'maxRunMs', max_run_ms,",
        "'containerized', containerized,",
        "'containerImage', container_image,",
        "'containerBuildStatus', container_build_status,",
        "'containerBuildError', container_build_error,",
        "'containerBuiltAt', container_built_at,",
        "'status', status,",
        "'labels', labels_json::jsonb,",
        "'metaData', meta_data_json::jsonb",
        ")::text ",
        "from (",
        SelectSql,
        ") as lambda_function_row ",
        "where ",
        identifier_where_clause(Kind, Identifier),
        " ",
        "and is_soft_deleted = false ",
        "limit 1"
    ]).

identifier_where_clause(uuid, Identifier) ->
    ["id = '", Identifier, "'"];
identifier_where_clause(slug, Identifier) ->
    ["slug = '", Identifier, "'"].

command_for_definition(FallbackCommand, DefinitionJson) ->
    Runtime = runtime_from_definition(DefinitionJson),
    case supported_runtime(Runtime) of
        false ->
            {error, iolist_to_binary(["unsupported lambda runtime: ", Runtime])};
        true ->
            case json_bool_field(DefinitionJson, <<"containerized">>, false) of
                true ->
                    container_command(Runtime, DefinitionJson);
                false ->
                    case host_runtime_allowed(Runtime) of
                        true ->
                            case host_command(Runtime) of
                                {ok, Command} -> {ok, Command};
                                {error, _Reason} -> {ok, FallbackCommand}
                            end;
                        false ->
                            {error, iolist_to_binary([
                                "lambda runtime requires containerized=true for host execution: ",
                                Runtime
                            ])}
                    end
            end
    end.

supported_runtime(Runtime) ->
    lists:member(Runtime, [<<"nodejs">>, <<"python3">>, <<"ruby">>, <<"bash">>]).

host_command(<<"nodejs">>) ->
    {ok, <<"env -i PATH=\"$PATH\" NODE_ENV=production node --permission --allow-net child-runtimes/js-function-runner.mjs">>};
host_command(<<"python3">>) ->
    {ok, <<"env -i PATH=\"$PATH\" PYTHONUNBUFFERED=1 python3 child-runtimes/python-function-runner.py">>};
host_command(<<"ruby">>) ->
    {ok, <<"env -i PATH=\"$PATH\" ruby child-runtimes/ruby-function-runner.rb">>};
host_command(<<"bash">>) ->
    {ok, <<"env -i PATH=\"$PATH\" node --permission --allow-net --allow-child-process child-runtimes/bash-function-runner.mjs">>};
host_command(Runtime) ->
    {error, iolist_to_binary(["unsupported lambda runtime: ", Runtime])}.

host_runtime_allowed(Runtime) ->
    lists:member(Runtime, csv_env("LAMBDA_ALLOW_HOST_RUNTIMES", <<"nodejs">>)).

container_command(Runtime, DefinitionJson) ->
    BuildStatus = json_string_field(DefinitionJson, <<"containerBuildStatus">>),
    Image0 = case BuildStatus of
        <<"built">> -> json_string_field(DefinitionJson, <<"containerImage">>);
        _ -> <<>>
    end,
    Image = case Image0 of
        <<>> -> default_container_image(Runtime);
        _ -> Image0
    end,
    case safe_container_image(Image) of
        true ->
            Namespace = env_binary("LAMBDA_CONTAINER_NAMESPACE", <<"k8s.io">>),
            Network = env_binary("LAMBDA_CONTAINER_NETWORK", <<"bridge">>),
            Memory = env_binary("LAMBDA_CONTAINER_MEMORY", <<"256m">>),
            Cpus = env_binary("LAMBDA_CONTAINER_CPUS", <<"0.50">>),
            case env_binary("LAMBDA_CONTAINER_RUNNER", <<"nerdctl">>) of
                <<"ctr">> ->
                    Ctr = env_binary("LAMBDA_CONTAINER_CTR", <<"/usr/local/bin/ctr">>),
                    MemoryBytes = env_binary("LAMBDA_CONTAINER_MEMORY_BYTES", <<"268435456">>),
                    {ok, ctr_container_command(Ctr, Namespace, Network, MemoryBytes, Cpus, Image, Runtime)};
                _ ->
                    Nerdctl = env_binary("LAMBDA_CONTAINER_NERDCTL", <<"/usr/local/bin/nerdctl">>),
                    {ok, nerdctl_container_command(Nerdctl, Namespace, Network, Memory, Cpus, Image)}
            end;
        false ->
            {error, <<"containerImage contains unsupported characters">>}
    end.

nerdctl_container_command(Nerdctl, Namespace, Network, Memory, Cpus, Image) ->
    iolist_to_binary([
        shell_word(Nerdctl),
        " -n ", shell_word(Namespace),
        " run --rm -i --pull=never --read-only",
        " --tmpfs /tmp:rw,noexec,nosuid,size=16m",
        " --network ", shell_word(Network),
        " --user 10001:10001",
        " --cap-drop ALL",
        " --security-opt no-new-privileges",
        " --pids-limit 64",
        " --ulimit nofile=64:64",
        " --memory ", shell_word(Memory),
        " --cpus ", shell_word(Cpus),
        " ", shell_word(Image)
    ]).

ctr_container_command(Ctr, Namespace, Network, MemoryBytes, Cpus, Image, Runtime) ->
    ContainerId = iolist_to_binary(["dd-lambda-", Runtime, "-$(date +%s%N)-$$"]),
    iolist_to_binary([
        shell_word(Ctr),
        " -n ", shell_word(Namespace),
        " run --rm",
        ctr_network_args(Network),
        " --read-only",
        " --mount type=tmpfs,dst=/tmp,options=rw:noexec:nosuid:size=16m",
        " --user 10001:10001",
        ctr_cap_drop_args(),
        " --seccomp",
        " --memory-limit ", shell_word(MemoryBytes),
        " --cpus ", shell_word(Cpus),
        " ", shell_word(Image),
        " ", ContainerId
    ]).

ctr_network_args(<<"none">>) -> "";
ctr_network_args(<<"host">>) -> " --net-host";
ctr_network_args(_Network) -> " --cni".

ctr_cap_drop_args() ->
    " --cap-drop CAP_AUDIT_WRITE --cap-drop CAP_CHOWN --cap-drop CAP_DAC_OVERRIDE"
    " --cap-drop CAP_FOWNER --cap-drop CAP_FSETID --cap-drop CAP_KILL"
    " --cap-drop CAP_MKNOD --cap-drop CAP_NET_BIND_SERVICE --cap-drop CAP_NET_RAW"
    " --cap-drop CAP_SETFCAP --cap-drop CAP_SETGID --cap-drop CAP_SETPCAP"
    " --cap-drop CAP_SETUID --cap-drop CAP_SYS_CHROOT".

default_container_image(<<"nodejs">>) ->
    env_binary("LAMBDA_NODEJS_CONTAINER_IMAGE", <<"docker.io/library/dd-lambda-nodejs-runtime:dev">>);
default_container_image(<<"python3">>) ->
    env_binary("LAMBDA_PYTHON3_CONTAINER_IMAGE", <<"docker.io/library/dd-lambda-python3-runtime:dev">>);
default_container_image(<<"ruby">>) ->
    env_binary("LAMBDA_RUBY_CONTAINER_IMAGE", <<"docker.io/library/dd-lambda-ruby-runtime:dev">>);
default_container_image(<<"bash">>) ->
    env_binary("LAMBDA_BASH_CONTAINER_IMAGE", <<"docker.io/library/dd-lambda-bash-runtime:dev">>);
default_container_image(_Runtime) ->
    <<>>.

worker_key(Identifier, DefinitionJson, Runtime, Containerized) ->
    case json_string_field(DefinitionJson, <<"reuseKey">>) of
        <<>> ->
            {ok, case Containerized of
                true -> iolist_to_binary(["pool:container:", Runtime]);
                false -> iolist_to_binary(["pool:host:", Runtime])
            end};
        ReuseKey ->
            case safe_reuse_key(ReuseKey) of
                true -> {ok, iolist_to_binary(["function:", Identifier, ":", ReuseKey])};
                false -> {error, <<"reuseKey contains unsupported characters">>}
            end
    end.

idle_ms_from_definition(DefinitionJson, Fallback) ->
    Seconds = json_int_field(DefinitionJson, <<"idleTimeoutSeconds">>, 0),
    case Seconds > 0 of
        true -> max_int(Seconds * 1000, 1000);
        false -> max_int(Fallback, 1000)
    end.

timeout_ms_from_definition(DefinitionJson, Fallback) ->
    Timeout = json_int_field(DefinitionJson, <<"maxRunMs">>, 0),
    case Timeout > 0 of
        true -> max_int(Timeout, 1000);
        false -> max_int(Fallback, 1000)
    end.

runtime_from_definition(DefinitionJson) ->
    canonical_runtime(json_string_field(DefinitionJson, <<"runtime">>)).

canonical_runtime(<<"javascript">>) -> <<"nodejs">>;
canonical_runtime(<<"typescript">>) -> <<"nodejs">>;
canonical_runtime(<<"node">>) -> <<"nodejs">>;
canonical_runtime(<<"nodejs">>) -> <<"nodejs">>;
canonical_runtime(<<"python">>) -> <<"python3">>;
canonical_runtime(<<"python3">>) -> <<"python3">>;
canonical_runtime(<<"shell">>) -> <<"bash">>;
canonical_runtime(<<"bash">>) -> <<"bash">>;
canonical_runtime(<<"ruby">>) -> <<"ruby">>;
canonical_runtime(<<>>) -> <<"nodejs">>;
canonical_runtime(Runtime) -> Runtime.

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
            prewarm_workers(),
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
        {'DOWN', Monitor, process, _Pid, _Reason} ->
            delete_worker_by_monitor(Monitor),
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

prewarm_workers() ->
    HostRuntimes = lists:filter(
        fun host_runtime_allowed/1,
        csv_env("LAMBDA_PREWARM_RUNTIMES", <<"nodejs">>)
    ),
    lists:foreach(
        fun(Runtime) ->
            case host_command(Runtime) of
                {ok, Command} ->
                    case ensure_worker_in_manager(Command, iolist_to_binary(["pool:host:", Runtime]), 300000) of
                        {ok, _Pid} -> ok;
                        {error, Reason} ->
                            io:format(
                                "lambda prewarm host runtime=~s failed: ~s~n",
                                [safe_label(Runtime), safe_label(Reason)]
                            )
                    end;
                {error, Reason} ->
                    io:format(
                        "lambda prewarm host runtime=~s unsupported: ~s~n",
                        [safe_label(Runtime), safe_label(Reason)]
                    )
            end
        end,
        HostRuntimes
    ),
    ContainerRuntimes = csv_env("LAMBDA_PREWARM_CONTAINER_RUNTIMES", <<>>),
    lists:foreach(
        fun(Runtime) ->
            DefinitionJson = iolist_to_binary([
                "{\"runtime\":\"", Runtime, "\",\"containerized\":true,\"containerImage\":\"",
                default_container_image(Runtime),
                "\"}"
            ]),
            case container_command(Runtime, DefinitionJson) of
                {ok, Command} ->
                    case ensure_worker_in_manager(Command, iolist_to_binary(["pool:container:", Runtime]), 300000) of
                        {ok, _Pid} -> ok;
                        {error, Reason} ->
                            io:format(
                                "lambda prewarm container runtime=~s failed: ~s~n",
                                [safe_label(Runtime), safe_label(Reason)]
                            )
                    end;
                {error, Reason} ->
                    io:format(
                        "lambda prewarm container runtime=~s unsupported: ~s~n",
                        [safe_label(Runtime), safe_label(Reason)]
                    )
            end
        end,
        ContainerRuntimes
    ).

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
            Monitor = erlang:monitor(process, Pid),
            ets:insert(?WORKERS, {
                ReuseKey,
                #{
                    command => Command,
                    pid => Pid,
                    monitor => Monitor,
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
    ShellCommand = "exec " ++ binary_to_list(Command),
    try open_port({spawn_executable, "/bin/sh"}, [
        binary,
        exit_status,
        use_stdio,
        {args, ["-c", ShellCommand]}
    ]) of
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
            worker_receive_result(Port, From, Ref, <<>>);
        {Port, {exit_status, _Status}} ->
            ok;
        stop ->
            close_port(Port)
    end.

worker_receive_result(Port, From, Ref, Buffer) ->
    receive
        {Port, {data, Data}} ->
            NewBuffer = <<Buffer/binary, Data/binary>>,
            case byte_size(NewBuffer) > 1048576 of
                true ->
                    From ! {Ref, {error, <<"lambda child result exceeded byte limit">>}};
                false ->
                    case binary:match(NewBuffer, <<"\n">>) of
                        {Index, _Length} ->
                            Result = binary:part(NewBuffer, 0, Index),
                            From ! {Ref, {ok, Result}},
                            worker_loop(Port);
                        nomatch ->
                            worker_receive_result(Port, From, Ref, NewBuffer)
                    end
            end;
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
    case ets:lookup(?WORKERS, ReuseKey) of
        [{ReuseKey, Worker}] ->
            demonitor_worker(Worker),
            ets:delete(?WORKERS, ReuseKey);
        [] ->
            ok
    end.

delete_worker_by_monitor(Monitor) ->
    lists:foreach(
        fun({ReuseKey, Worker}) ->
            case maps:get(monitor, Worker, undefined) of
                Monitor -> ets:delete(?WORKERS, ReuseKey);
                _Other -> ok
            end
        end,
        ets:tab2list(?WORKERS)
    ).

demonitor_worker(Worker) ->
    case maps:get(monitor, Worker, undefined) of
        undefined -> ok;
        Monitor -> erlang:demonitor(Monitor, [flush])
    end.

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
    Slash = binary:replace(Value, <<"\\">>, <<"\\\\">>, [global]),
    Quote = binary:replace(Slash, <<"\"">>, <<"\\\"">>, [global]),
    Newline = binary:replace(Quote, <<"\n">>, <<"\\n">>, [global]),
    Return = binary:replace(Newline, <<"\r">>, <<"\\r">>, [global]),
    binary:replace(Return, <<"\t">>, <<"\\t">>, [global]).

json_string_field(Json0, Field0) ->
    Json = to_binary(Json0),
    Field = to_binary(Field0),
    Pattern = iolist_to_binary(["\"", Field, "\"\\s*:\\s*\"((?:\\\\.|[^\"])*)\""]),
    case re:run(Json, Pattern, [{capture, [1], binary}]) of
        {match, [Value]} -> json_unescape_string(Value);
        nomatch -> <<>>
    end.

json_bool_field(Json0, Field0, Default) ->
    Json = to_binary(Json0),
    Field = to_binary(Field0),
    Pattern = iolist_to_binary(["\"", Field, "\"\\s*:\\s*(true|false)"]),
    case re:run(Json, Pattern, [{capture, [1], binary}]) of
        {match, [<<"true">>]} -> true;
        {match, [<<"false">>]} -> false;
        nomatch -> Default
    end.

json_int_field(Json0, Field0, Default) ->
    Json = to_binary(Json0),
    Field = to_binary(Field0),
    Pattern = iolist_to_binary(["\"", Field, "\"\\s*:\\s*([0-9]+)"]),
    case re:run(Json, Pattern, [{capture, [1], binary}]) of
        {match, [Value]} ->
            case string:to_integer(binary_to_list(Value)) of
                {Int, _Rest} -> Int;
                _ -> Default
            end;
        nomatch ->
            Default
    end.

json_unescape_string(Value0) ->
    Value1 = binary:replace(Value0, <<"\\\"">>, <<"\"">>, [global]),
    Value2 = binary:replace(Value1, <<"\\\\">>, <<"\\">>, [global]),
    Value2.

env_binary(Name, Default) ->
    case os:getenv(Name) of
        false -> Default;
        "" -> Default;
        Value -> to_binary(Value)
    end.

csv_env(Name, Default) ->
    Raw = env_binary(Name, Default),
    Tokens = string:tokens(binary_to_list(Raw), ","),
    lists:filtermap(
        fun(Token0) ->
            Trimmed = to_binary(string:trim(Token0)),
            case Trimmed of
                <<>> -> false;
                _ -> {true, canonical_runtime(Trimmed)}
            end
        end,
        Tokens
    ).

safe_container_image(Image) ->
    re:run(Image, "^[A-Za-z0-9][A-Za-z0-9._:/@-]{0,511}$", [{capture, none}]) =:= match.

safe_reuse_key(ReuseKey) ->
    re:run(ReuseKey, "^[A-Za-z0-9][A-Za-z0-9._:-]{0,119}$", [{capture, none}]) =:= match.

shell_word(Value0) ->
    Value = to_binary(Value0),
    Escaped = binary:replace(Value, <<"\'">>, <<"\'\"'\"\'">>, [global]),
    iolist_to_binary(["'", Escaped, "'"]).

safe_label(Value) ->
    binary_to_list(binary:replace(Value, <<"\"">>, <<"">>, [global])).
