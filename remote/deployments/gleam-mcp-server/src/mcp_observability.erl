-module(mcp_observability).

-export([snapshot/0]).

-define(DEFAULT_HTTP_TIMEOUT_MS, 650).
-define(DEFAULT_SNAPSHOT_TIMEOUT_MS, 1800).
-define(DEFAULT_MAX_SNIPPET_BYTES, 768).

snapshot() ->
    ensure_http_client(),
    Targets = targets(),
    SnapshotTimeoutMs = snapshot_timeout_ms(),
    TargetJobs = start_target_jobs(Targets),
    TargetResults = collect_target_jobs(TargetJobs, SnapshotTimeoutMs),
    TargetJsons = lists:map(
        fun({Target, Ref}) ->
            case maps:find(Ref, TargetResults) of
                {ok, Json} ->
                    Json;
                error ->
                    target_unavailable_json(
                        Target,
                        <<"snapshot timeout">>,
                        SnapshotTimeoutMs
                    )
            end
        end,
        TargetJobs
    ),
    iolist_to_binary([
        "{\"generatedAtMs\":",
        integer_to_binary(now_ms()),
        ",\"mode\":\"read-only-internal-service-dns\",",
        "\"httpTimeoutMs\":",
        integer_to_binary(http_timeout_ms()),
        ",\"snapshotTimeoutMs\":",
        integer_to_binary(SnapshotTimeoutMs),
        ",\"snippetBytes\":",
        integer_to_binary(max_snippet_bytes()),
        ",\"targetsCount\":",
        integer_to_binary(length(Targets)),
        ",",
        "\"targets\":[",
        join_json(TargetJsons),
        "]}"
    ]).

start_target_jobs(Targets) ->
    Parent = self(),
    lists:map(
        fun(Target) ->
            Ref = make_ref(),
            spawn(fun() ->
                Parent ! {mcp_observability_target, Ref, safe_target_json(Target)}
            end),
            {Target, Ref}
        end,
        Targets
    ).

collect_target_jobs(TargetJobs, TimeoutMs) ->
    PendingRefs = lists:map(fun({_Target, Ref}) -> Ref end, TargetJobs),
    Deadline = erlang:monotonic_time(millisecond) + TimeoutMs,
    collect_target_jobs(PendingRefs, Deadline, #{}).

collect_target_jobs([], _Deadline, Results) ->
    Results;
collect_target_jobs(PendingRefs, Deadline, Results) ->
    RemainingMs = max(0, Deadline - erlang:monotonic_time(millisecond)),
    receive
        {mcp_observability_target, Ref, Json} ->
            case lists:member(Ref, PendingRefs) of
                true ->
                    collect_target_jobs(
                        lists:delete(Ref, PendingRefs),
                        Deadline,
                        maps:put(Ref, Json, Results)
                    );
                false ->
                    collect_target_jobs(PendingRefs, Deadline, Results)
            end
    after RemainingMs ->
        Results
    end.

safe_target_json(Target) ->
    try target_json(Target) of
        Json -> Json
    catch
        Class:Reason ->
            target_unavailable_json(
                Target,
                to_binary(io_lib:format("~p:~p", [Class, Reason])),
                0
            )
    end.

targets() ->
    [
        target(
            <<"prometheus">>,
            <<"metrics-store">>,
            env_binary("MCP_PROMETHEUS_URL", <<"http://dd-prometheus.observability.svc.cluster.local:9090">>),
            <<"/-/ready">>,
            <<"/api/v1/query?query=up">>
        ),
        target(
            <<"loki">>,
            <<"logs-store">>,
            env_binary("MCP_LOKI_URL", <<"http://dd-loki.observability.svc.cluster.local:3100">>),
            <<"/ready">>,
            <<"/loki/api/v1/labels">>
        ),
        target(
            <<"grafana">>,
            <<"dashboard-ui">>,
            env_binary("MCP_GRAFANA_URL", <<"http://dd-grafana.observability.svc.cluster.local:3000">>),
            <<"/api/health">>,
            <<"/api/health">>
        ),
        target(
            <<"otel-collector">>,
            <<"collector-metrics">>,
            env_binary("MCP_OTEL_COLLECTOR_URL", <<"http://dd-otel-collector.observability.svc.cluster.local:8889">>),
            <<"/metrics">>,
            <<"/metrics">>
        ),
        target(
            <<"tempo">>,
            <<"trace-store">>,
            env_binary("MCP_TEMPO_URL", <<"http://dd-tempo.observability.svc.cluster.local:3200">>),
            <<"/ready">>,
            <<"/ready">>
        ),
        target(
            <<"jaeger">>,
            <<"trace-query">>,
            env_binary("MCP_JAEGER_URL", <<"http://dd-jaeger.observability.svc.cluster.local:16686">>),
            <<"/">>,
            <<"/api/services">>
        ),
        target(
            <<"nats-exporter">>,
            <<"messaging-metrics">>,
            env_binary("MCP_NATS_METRICS_URL", <<"http://dd-nats.messaging.svc.cluster.local:7777">>),
            <<"/metrics">>,
            <<"/metrics">>
        )
    ].

target(Name, Kind, BaseUrl, HealthPath, ReadPath) ->
    #{
        name => Name,
        kind => Kind,
        base_url => trim_trailing_slash(BaseUrl),
        health_path => HealthPath,
        read_path => ReadPath
    }.

target_json(Target) ->
    BaseUrl = maps:get(base_url, Target),
    HealthPath = maps:get(health_path, Target),
    ReadPath = maps:get(read_path, Target),
    HealthUrl = join_url(BaseUrl, HealthPath),
    ReadUrl = join_url(BaseUrl, ReadPath),
    {Health, Read} = case HealthUrl =:= ReadUrl of
        true ->
            HealthRead = http_get(HealthUrl),
            {HealthRead, HealthRead};
        false ->
            read_health_and_data(HealthUrl, ReadUrl)
    end,
    target_result_json(Target, Health, Read).

read_health_and_data(HealthUrl, ReadUrl) ->
    Parent = self(),
    HealthRef = make_ref(),
    ReadRef = make_ref(),
    TimeoutMs = http_timeout_ms() + 100,
    spawn(fun() ->
        Parent ! {mcp_observability_http, HealthRef, safe_http_get(HealthUrl)}
    end),
    spawn(fun() ->
        Parent ! {mcp_observability_http, ReadRef, safe_http_get(ReadUrl)}
    end),
    {
        collect_http_result(HealthRef, TimeoutMs),
        collect_http_result(ReadRef, TimeoutMs)
    }.

collect_http_result(Ref, TimeoutMs) ->
    receive
        {mcp_observability_http, Ref, Response} ->
            Response
    after TimeoutMs ->
        response(0, <<>>, <<"http read timeout">>, TimeoutMs)
    end.

safe_http_get(Url) ->
    try http_get(Url) of
        Response -> Response
    catch
        Class:Reason ->
            response(0, <<>>, to_binary(io_lib:format("~p:~p", [Class, Reason])), 0)
    end.

target_unavailable_json(Target, Error, ElapsedMs) ->
    Unavailable = response(0, <<>>, Error, ElapsedMs),
    target_result_json(Target, Unavailable, Unavailable).

target_result_json(Target, Health, Read) ->
    BaseUrl = maps:get(base_url, Target),
    HealthPath = maps:get(health_path, Target),
    ReadPath = maps:get(read_path, Target),
    iolist_to_binary([
        "{\"name\":\"",
        json_escape(maps:get(name, Target)),
        "\",\"kind\":\"",
        json_escape(maps:get(kind, Target)),
        "\",\"baseUrl\":\"",
        json_escape(BaseUrl),
        "\",\"healthPath\":\"",
        json_escape(HealthPath),
        "\",\"readPath\":\"",
        json_escape(ReadPath),
        "\",\"health\":",
        response_json(Health),
        ",\"read\":",
        response_json(Read),
        "}"
    ]).

http_get(Url) ->
    Started = erlang:monotonic_time(millisecond),
    Request = {
        binary_to_list(Url),
        [
            {"accept", "*/*"},
            {"user-agent", "dd-gleam-mcp-server/observability"}
        ]
    },
    TimeoutMs = http_timeout_ms(),
    HttpOptions = [{timeout, TimeoutMs}, {connect_timeout, TimeoutMs}],
    Options = [{body_format, binary}],
    case httpc:request(get, Request, HttpOptions, Options) of
        {ok, {{_Version, Status, _Reason}, _Headers, Body}} ->
            response(Status, Body, <<>>, elapsed_ms(Started));
        {error, Reason} ->
            response(0, <<>>, to_binary(io_lib:format("~p", [Reason])), elapsed_ms(Started))
    end.

response(Status, Body, Error, ElapsedMs) ->
    #{
        status => Status,
        ok => Status >= 200 andalso Status < 400,
        elapsed_ms => ElapsedMs,
        body => snippet(Body),
        error => Error
    }.

response_json(Response) ->
    Status = maps:get(status, Response),
    Ok = maps:get(ok, Response),
    Error = maps:get(error, Response),
    Body = maps:get(body, Response),
    iolist_to_binary([
        "{\"ok\":",
        bool_json(Ok),
        ",\"status\":",
        integer_to_binary(Status),
        ",\"elapsedMs\":",
        integer_to_binary(maps:get(elapsed_ms, Response)),
        ",\"bodySnippet\":\"",
        json_escape(Body),
        "\"",
        error_json(Error),
        "}"
    ]).

error_json(<<>>) ->
    "";
error_json(Error) ->
    [",\"error\":\"", json_escape(Error), "\""].

join_url(BaseUrl, Path) ->
    iolist_to_binary([trim_trailing_slash(BaseUrl), ensure_leading_slash(Path)]).

ensure_leading_slash(<<"/", _/binary>> = Path) ->
    Path;
ensure_leading_slash(Path) ->
    <<"/", Path/binary>>.

trim_trailing_slash(<<>>) ->
    <<>>;
trim_trailing_slash(Value) ->
    case binary:last(Value) of
        $/ ->
            trim_trailing_slash(binary:part(Value, 0, byte_size(Value) - 1));
        _ ->
            Value
    end.

join_json([]) ->
    "";
join_json([One]) ->
    One;
join_json([One | Rest]) ->
    [One, ",", join_json(Rest)].

snippet(Body) ->
    MaxBytes = max_snippet_bytes(),
    case byte_size(Body) =< MaxBytes of
        true -> Body;
        _ ->
            binary:part(Body, 0, MaxBytes)
    end.

bool_json(true) ->
    "true";
bool_json(false) ->
    "false".

json_escape(Value0) ->
    Value = to_binary(Value0),
    Slash = binary:replace(Value, <<"\\">>, <<"\\\\">>, [global]),
    Quote = binary:replace(Slash, <<"\"">>, <<"\\\"">>, [global]),
    Newline = binary:replace(Quote, <<"\n">>, <<"\\n">>, [global]),
    Return = binary:replace(Newline, <<"\r">>, <<"\\r">>, [global]),
    binary:replace(Return, <<"\t">>, <<"\\t">>, [global]).

env_binary(Name, Default) ->
    case os:getenv(Name) of
        false -> Default;
        "" -> Default;
        Value -> to_binary(Value)
    end.

env_integer(Name, Default, Min, Max) ->
    case os:getenv(Name) of
        false -> Default;
        "" -> Default;
        Value ->
            case string:to_integer(Value) of
                {Int, _Rest} when is_integer(Int), Int >= Min ->
                    min(Int, Max);
                _ ->
                    Default
            end
    end.

http_timeout_ms() ->
    env_integer("MCP_OBS_HTTP_TIMEOUT_MS", ?DEFAULT_HTTP_TIMEOUT_MS, 100, 5000).

snapshot_timeout_ms() ->
    env_integer("MCP_OBS_SNAPSHOT_TIMEOUT_MS", ?DEFAULT_SNAPSHOT_TIMEOUT_MS, 250, 10000).

max_snippet_bytes() ->
    env_integer("MCP_OBS_SNIPPET_BYTES", ?DEFAULT_MAX_SNIPPET_BYTES, 128, 8192).

to_binary(Value) when is_binary(Value) ->
    Value;
to_binary(Value) when is_list(Value) ->
    unicode:characters_to_binary(Value);
to_binary(Value) ->
    unicode:characters_to_binary(io_lib:format("~p", [Value])).

ensure_http_client() ->
    application:ensure_all_started(inets),
    ok.

elapsed_ms(Started) ->
    erlang:monotonic_time(millisecond) - Started.

now_ms() ->
    erlang:system_time(millisecond).
