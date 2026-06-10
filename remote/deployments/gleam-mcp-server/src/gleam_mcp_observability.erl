-module(gleam_mcp_observability).

-export([
    targets_json/0,
    health_json/0,
    telemetry_summary_json/0,
    prometheus_up_json/0,
    loki_labels_json/0,
    grafana_inventory_json/0,
    nats_metrics_json/0,
    trace_backends_json/0
]).

-define(MAX_TIMEOUT_MS, 5000).
-define(MAX_BODY_LIMIT_BYTES, 262144).

targets_json() ->
    json_obj([
        {<<"service">>, <<"dd-gleam-mcp-server">>},
        {<<"mode">>, <<"read-only">>},
        {<<"timeoutMs">>, timeout_ms()},
        {<<"bodyLimitBytes">>, body_limit_bytes()},
        {<<"targets">>, raw(json_arr([
            target_obj(<<"prometheus">>, prometheus_url(<<>>), <<"metrics/query store">>),
            target_obj(<<"loki">>, loki_url(<<>>), <<"log store">>),
            target_obj(<<"grafana">>, grafana_url(<<>>), <<"dashboard UI/API">>),
            target_obj(<<"tempo">>, tempo_url(<<>>), <<"trace store">>),
            target_obj(<<"jaeger">>, jaeger_url(<<>>), <<"trace query UI/API">>),
            target_obj(<<"otelCollectorPrometheus">>, otel_url(<<"/metrics">>), <<"collector exported metrics">>),
            target_obj(<<"natsMonitor">>, nats_monitor_url(<<>>), <<"NATS server monitoring API">>),
            target_obj(<<"natsMetrics">>, nats_metrics_url(<<"/metrics">>), <<"NATS Prometheus exporter">>)
        ]))},
        {<<"safeQueries">>, raw(json_arr([
            json_obj([{<<"name">>, <<"prometheus_up">>}, {<<"query">>, <<"up">>}]),
            json_obj([{<<"name">>, <<"loki_labels">>}, {<<"path">>, <<"/loki/api/v1/labels">>}]),
            json_obj([{<<"name">>, <<"grafana_datasources">>}, {<<"path">>, <<"/api/datasources">>}]),
            json_obj([{<<"name">>, <<"grafana_dashboards">>}, {<<"path">>, <<"/api/search?type=dash-db">>}]),
            json_obj([{<<"name">>, <<"jaeger_services">>}, {<<"path">>, <<"/api/services">>}]),
            json_obj([{<<"name">>, <<"nats_varz">>}, {<<"path">>, <<"/varz">>}]),
            json_obj([{<<"name">>, <<"nats_exporter_metrics">>}, {<<"path">>, <<"/metrics">>}])
        ]))}
    ]).

health_json() ->
    json_obj([
        {<<"service">>, <<"dd-gleam-mcp-server">>},
        {<<"mode">>, <<"read-only observability health">>},
        {<<"checks">>, raw(parallel_checks([
            {<<"prometheus">>, prometheus_url(<<"/-/healthy">>), 2048},
            {<<"loki">>, loki_url(<<"/ready">>), 2048},
            {<<"grafana">>, grafana_url(<<"/api/health">>), 4096},
            {<<"tempo">>, tempo_url(<<"/ready">>), 2048},
            {<<"jaeger">>, jaeger_url(<<"/api/services">>), 4096},
            {<<"otelCollectorMetrics">>, otel_url(<<"/metrics">>), 4096},
            {<<"natsMonitor">>, nats_monitor_url(<<"/healthz">>), 2048},
            {<<"natsMetrics">>, nats_metrics_url(<<"/metrics">>), 4096}
        ]))}
    ]).

telemetry_summary_json() ->
    json_obj([
        {<<"service">>, <<"dd-gleam-mcp-server">>},
        {<<"mode">>, <<"bounded read-only telemetry summary">>},
        {<<"sources">>, raw(parallel_checks([
            {<<"prometheusHealthy">>, prometheus_url(<<"/-/healthy">>), 2048},
            {<<"prometheusTargets">>, prometheus_url(<<"/api/v1/targets?state=active">>), body_limit_bytes()},
            {<<"lokiReady">>, loki_url(<<"/ready">>), 2048},
            {<<"lokiLabels">>, loki_url(<<"/loki/api/v1/labels">>), body_limit_bytes()},
            {<<"grafanaHealth">>, grafana_url(<<"/api/health">>), 4096},
            {<<"grafanaDatasources">>, grafana_url(<<"/api/datasources">>), body_limit_bytes()},
            {<<"grafanaDashboards">>, grafana_url(<<"/api/search?type=dash-db">>), body_limit_bytes()},
            {<<"tempoReady">>, tempo_url(<<"/ready">>), 2048},
            {<<"jaegerServices">>, jaeger_url(<<"/api/services">>), body_limit_bytes()},
            {<<"otelCollectorMetrics">>, otel_url(<<"/metrics">>), body_limit_bytes()},
            {<<"natsVarz">>, nats_monitor_url(<<"/varz">>), body_limit_bytes()},
            {<<"natsExporterMetrics">>, nats_metrics_url(<<"/metrics">>), body_limit_bytes()}
        ]))}
    ]).

prometheus_up_json() ->
    Query = <<"up">>,
    Url = prometheus_url(<<"/api/v1/query?query=", Query/binary>>),
    json_obj([
        {<<"service">>, <<"dd-gleam-mcp-server">>},
        {<<"source">>, <<"prometheus">>},
        {<<"query">>, Query},
        {<<"result">>, raw(http_result(Url, body_limit_bytes()))}
    ]).

loki_labels_json() ->
    Url = loki_url(<<"/loki/api/v1/labels">>),
    json_obj([
        {<<"service">>, <<"dd-gleam-mcp-server">>},
        {<<"source">>, <<"loki">>},
        {<<"path">>, <<"/loki/api/v1/labels">>},
        {<<"result">>, raw(http_result(Url, body_limit_bytes()))}
    ]).

grafana_inventory_json() ->
    json_obj([
        {<<"service">>, <<"dd-gleam-mcp-server">>},
        {<<"source">>, <<"grafana">>},
        {<<"datasources">>, raw(http_result(grafana_url(<<"/api/datasources">>), body_limit_bytes()))},
        {<<"dashboards">>, raw(http_result(grafana_url(<<"/api/search?type=dash-db">>), body_limit_bytes()))}
    ]).

nats_metrics_json() ->
    json_obj([
        {<<"service">>, <<"dd-gleam-mcp-server">>},
        {<<"source">>, <<"nats">>},
        {<<"monitor">>, raw(http_result(nats_monitor_url(<<"/varz">>), body_limit_bytes()))},
        {<<"metrics">>, raw(http_result(nats_metrics_url(<<"/metrics">>), body_limit_bytes()))}
    ]).

trace_backends_json() ->
    json_obj([
        {<<"service">>, <<"dd-gleam-mcp-server">>},
        {<<"mode">>, <<"trace backend read-only summary">>},
        {<<"checks">>, raw(parallel_checks([
            {<<"tempoReady">>, tempo_url(<<"/ready">>), 4096},
            {<<"jaegerServices">>, jaeger_url(<<"/api/services">>), body_limit_bytes()}
        ]))}
    ]).

target_obj(Name, Url, Role) ->
    json_obj([{<<"name">>, Name}, {<<"url">>, gleam_mcp_redact:url_for_output(Url)}, {<<"role">>, Role}]).

check(Name, Url, Limit) ->
    Result = http_result(Url, Limit),
    json_obj([{<<"name">>, Name}, {<<"url">>, gleam_mcp_redact:url_for_output(Url)}, {<<"result">>, raw(Result)}]).

parallel_checks(Specs) ->
    Ref = make_ref(),
    Parent = self(),
    Indexed = zip_index(Specs, 1),
    MaxWait = timeout_ms() + 1000,
    Deadline = erlang:monotonic_time(millisecond) + MaxWait,
    lists:foreach(
        fun({Index, {Name, Url, Limit}}) ->
            spawn(fun() -> Parent ! {Ref, Index, check(Name, Url, Limit)} end)
        end,
        Indexed
    ),
    Results0 = collect_checks(Ref, length(Indexed), [], Deadline),
    Seen = [Index || {Index, _Result} <- Results0],
    Missing = [
        {Index, timeout_check(Name, Url, MaxWait)}
        || {Index, {Name, Url, _Limit}} <- Indexed,
           not lists:member(Index, Seen)
    ],
    json_arr([Result || {_Index, Result} <- lists:sort(Results0 ++ Missing)]).

timeout_check(Name, Url, Timeout) ->
    json_obj([
        {<<"name">>, Name},
        {<<"url">>, Url},
        {<<"result">>, raw(json_obj([
            {<<"ok">>, false},
            {<<"durationMs">>, Timeout},
            {<<"error">>, <<"mcp observability check timed out">>}
        ]))}
    ]).

zip_index([], _Index) ->
    [];
zip_index([Item | Rest], Index) ->
    [{Index, Item} | zip_index(Rest, Index + 1)].

collect_checks(_Ref, 0, Acc, _Timeout) ->
    Acc;
collect_checks(Ref, Remaining, Acc, Deadline) ->
    Wait = Deadline - erlang:monotonic_time(millisecond),
    case Wait > 0 of
        false ->
            Acc;
        true ->
            receive
                {Ref, Index, Result} ->
                    collect_checks(Ref, Remaining - 1, [{Index, Result} | Acc], Deadline)
            after Wait ->
                Acc
            end
    end.

http_result(UrlBin, Limit) ->
    _ = application:ensure_all_started(inets),
    _ = application:ensure_all_started(ssl),
    Timeout = timeout_ms(),
    Url = binary_to_list(UrlBin),
    Headers = [{"accept", "application/json,text/plain,*/*"}],
    HttpOptions = [{timeout, Timeout}, {connect_timeout, Timeout}],
    Options = [{body_format, binary}],
    Started = erlang:monotonic_time(millisecond),
    case catch httpc:request(get, {Url, Headers}, HttpOptions, Options) of
        {ok, {{_, Status, Reason}, _RespHeaders, Body0}} ->
            Duration = erlang:monotonic_time(millisecond) - Started,
            Body = gleam_mcp_redact:sample(to_binary(Body0), Limit),
            json_obj([
                {<<"ok">>, Status >= 200 andalso Status < 400},
                {<<"status">>, Status},
                {<<"reason">>, to_binary(Reason)},
                {<<"durationMs">>, Duration},
                {<<"sample">>, Body}
            ]);
        {error, Reason} ->
            Duration = erlang:monotonic_time(millisecond) - Started,
            json_obj([
                {<<"ok">>, false},
                {<<"durationMs">>, Duration},
                {<<"error">>, to_binary(io_lib:format("~p", [Reason]))}
            ]);
        {'EXIT', Reason} ->
            Duration = erlang:monotonic_time(millisecond) - Started,
            json_obj([
                {<<"ok">>, false},
                {<<"durationMs">>, Duration},
                {<<"error">>, to_binary(io_lib:format("~p", [Reason]))}
            ])
    end.

prometheus_url(Path) ->
    join_url(safe_base_url("MCP_PROMETHEUS_URL", <<"http://dd-prometheus.observability.svc.cluster.local:9090">>), Path).

loki_url(Path) ->
    join_url(safe_base_url("MCP_LOKI_URL", <<"http://dd-loki.observability.svc.cluster.local:3100">>), Path).

grafana_url(Path) ->
    join_url(safe_base_url("MCP_GRAFANA_URL", <<"http://dd-grafana.observability.svc.cluster.local:3000">>), Path).

tempo_url(Path) ->
    join_url(safe_base_url("MCP_TEMPO_URL", <<"http://dd-tempo.observability.svc.cluster.local:3200">>), Path).

jaeger_url(Path) ->
    join_url(safe_base_url("MCP_JAEGER_URL", <<"http://dd-jaeger.observability.svc.cluster.local:16686">>), Path).

otel_url(Path) ->
    join_url(safe_base_url("MCP_OTEL_COLLECTOR_URL", <<"http://dd-otel-collector.observability.svc.cluster.local:8889">>), Path).

nats_monitor_url(Path) ->
    join_url(safe_base_url("MCP_NATS_MONITOR_URL", <<"http://dd-nats.messaging.svc.cluster.local:8222">>), Path).

nats_metrics_url(Path) ->
    join_url(safe_base_url("MCP_NATS_METRICS_URL", <<"http://dd-nats.messaging.svc.cluster.local:7777">>), Path).

join_url(Base0, Path0) ->
    Base = trim_trailing_slash(Base0),
    Path = case Path0 of
        <<>> -> <<>>;
        <<"/", _/binary>> -> Path0;
        _ -> <<"/", Path0/binary>>
    end,
    <<Base/binary, Path/binary>>.

trim_trailing_slash(<<>>) -> <<>>;
trim_trailing_slash(Bin) ->
    Size = byte_size(Bin),
    case binary:part(Bin, Size - 1, 1) of
        <<"/">> -> trim_trailing_slash(binary:part(Bin, 0, Size - 1));
        _ -> Bin
    end.

timeout_ms() ->
    bounded_int("MCP_OBSERVABILITY_TIMEOUT_MS", 1500, 100, ?MAX_TIMEOUT_MS).

body_limit_bytes() ->
    bounded_int("MCP_OBSERVABILITY_BODY_LIMIT_BYTES", 20000, 1024, ?MAX_BODY_LIMIT_BYTES).

safe_base_url(Name, Default) ->
    gleam_mcp_redact:safe_base_url(Name, Default).

bounded_int(Name, Default, Min, Max) ->
    gleam_mcp_redact:bounded_int(Name, Default, Min, Max).

json_obj(Pairs) ->
    <<"{", (join([json_pair(K, V) || {K, V} <- Pairs], <<",">>))/binary, "}">>.

json_pair(Key, Value) ->
    <<(json_string(Key))/binary, ":", (json_value(Value))/binary>>.

json_arr(Items) ->
    <<"[", (join(Items, <<",">>))/binary, "]">>.

json_value(Value) when is_binary(Value) -> json_string(Value);
json_value({raw, Value}) -> Value;
json_value(Value) when is_integer(Value) -> integer_to_binary(Value);
json_value(true) -> <<"true">>;
json_value(false) -> <<"false">>;
json_value(Value) when is_list(Value) -> to_binary(Value).

json_string(Value0) ->
    Value = to_binary(Value0),
    <<"\"", (json_escape(Value))/binary, "\"">>.

json_escape(Value0) ->
    Slash = binary:replace(Value0, <<"\\">>, <<"\\\\">>, [global]),
    Quote = binary:replace(Slash, <<"\"">>, <<"\\\"">>, [global]),
    Newline = binary:replace(Quote, <<"\n">>, <<"\\n">>, [global]),
    Return = binary:replace(Newline, <<"\r">>, <<"\\r">>, [global]),
    Tab = binary:replace(Return, <<"\t">>, <<"\\t">>, [global]),
    binary:replace(Tab, <<"\b">>, <<"\\b">>, [global]).

join([], _Sep) -> <<>>;
join([One], _Sep) -> One;
join([First | Rest], Sep) ->
    <<First/binary, Sep/binary, (join(Rest, Sep))/binary>>.

raw(Value) ->
    {raw, Value}.

to_binary(Value) when is_binary(Value) -> Value;
to_binary(Value) when is_list(Value) -> unicode:characters_to_binary(Value);
to_binary(Value) when is_atom(Value) -> atom_to_binary(Value, utf8);
to_binary(Value) -> unicode:characters_to_binary(io_lib:format("~p", [Value])).
