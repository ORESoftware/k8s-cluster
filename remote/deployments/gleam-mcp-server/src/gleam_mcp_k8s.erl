-module(gleam_mcp_k8s).

-export([deployments_json/0, human_access_policy_json/0, inventory_json/0]).

-define(DEFAULT_API_URL, <<"https://kubernetes.default.svc">>).
-define(DEFAULT_DEPLOYMENTS_PATH, <<"/apis/apps/v1/deployments?limit=500">>).
-define(DEFAULT_TOKEN_PATH, <<"/var/run/secrets/kubernetes.io/serviceaccount/token">>).
-define(DEFAULT_CA_PATH, <<"/var/run/secrets/kubernetes.io/serviceaccount/ca.crt">>).
-define(DEFAULT_TIMEOUT_MS, 1500).
-define(DEFAULT_BODY_LIMIT_BYTES, 262144).
-define(DEFAULT_INVENTORY_BODY_LIMIT_BYTES, 32768).
-define(MAX_TIMEOUT_MS, 5000).
-define(MAX_BODY_LIMIT_BYTES, 262144).

deployments_json() ->
    Base = gleam_mcp_redact:safe_base_url("MCP_KUBERNETES_API_URL", ?DEFAULT_API_URL),
    Limit = bounded_int("MCP_KUBERNETES_BODY_LIMIT_BYTES", ?DEFAULT_BODY_LIMIT_BYTES, 1024, ?MAX_BODY_LIMIT_BYTES),
    Path = deployments_path(),
    resource_json(Base, Limit, <<"deployments">>, <<"all-namespaces">>, Path).

inventory_json() ->
    Base = gleam_mcp_redact:safe_base_url("MCP_KUBERNETES_API_URL", ?DEFAULT_API_URL),
    Limit = bounded_int(
        "MCP_KUBERNETES_INVENTORY_BODY_LIMIT_BYTES",
        ?DEFAULT_INVENTORY_BODY_LIMIT_BYTES,
        1024,
        ?MAX_BODY_LIMIT_BYTES
    ),
    Resources = [resource_entry(Base, Limit, Target) || Target <- inventory_targets()],
    json_obj([
        {<<"source">>, <<"kubernetes-api">>},
        {<<"scope">>, <<"cluster inventory metadata">>},
        {<<"readOnly">>, true},
        {<<"metadataOnlyRequest">>, true},
        {<<"resources">>, {array, Resources}},
        {<<"excluded">>, {array, [
            <<"secrets">>,
            <<"configmaps data">>,
            <<"pods/exec">>,
            <<"pods/log">>,
            <<"mutation verbs">>
        ]}}
    ]).

human_access_policy_json() ->
    json_obj([
        {<<"source">>, <<"mcp-policy">>},
        {<<"readOnlyByDefault">>, true},
        {<<"humanAuthRequiredForPublicGateway">>, true},
        {<<"humanAuthPath">>, <<"/auth?return=/mcp/home">>},
        {<<"acceptedGatewayProofs">>, {array, [
            <<"dd_auth HttpOnly cookie from dd-remote-auth">>,
            <<"Auth header with the gateway secret for non-browser callers">>
        ]}},
        {<<"recommendedHumanProof">>, <<"operator passphrase plus optional TOTP on dd-remote-auth">>},
        {<<"elevatedMcpToolsEnabled">>, false},
        {<<"sensitiveKubernetesAccess">>, {array, [
            <<"Do not expose Kubernetes Secrets through MCP">>,
            <<"Do not expose pod exec through MCP">>,
            <<"Use VPN plus dd-bastion or SSM/SSH for human shell access">>
        ]}},
        {<<"auditExpectation">>, <<"Add a separate short-lived grant service before enabling any write, secret, log, or exec-capable MCP tool.">>}
    ]).

inventory_targets() ->
    [
        {<<"namespaces">>, <<"cluster">>, <<"/api/v1/namespaces?limit=500">>},
        {<<"nodes">>, <<"cluster">>, <<"/api/v1/nodes?limit=500">>},
        {<<"persistentvolumes">>, <<"cluster">>, <<"/api/v1/persistentvolumes?limit=500">>},
        {<<"serviceaccounts">>, <<"all-namespaces">>, <<"/api/v1/serviceaccounts?limit=500">>},
        {<<"pods">>, <<"all-namespaces">>, <<"/api/v1/pods?limit=500">>},
        {<<"services">>, <<"all-namespaces">>, <<"/api/v1/services?limit=500">>},
        {<<"endpoints">>, <<"all-namespaces">>, <<"/api/v1/endpoints?limit=500">>},
        {<<"persistentvolumeclaims">>, <<"all-namespaces">>, <<"/api/v1/persistentvolumeclaims?limit=500">>},
        {<<"events">>, <<"all-namespaces">>, <<"/api/v1/events?limit=500">>},
        {<<"deployments">>, <<"all-namespaces">>, <<"/apis/apps/v1/deployments?limit=500">>},
        {<<"daemonsets">>, <<"all-namespaces">>, <<"/apis/apps/v1/daemonsets?limit=500">>},
        {<<"replicasets">>, <<"all-namespaces">>, <<"/apis/apps/v1/replicasets?limit=500">>},
        {<<"statefulsets">>, <<"all-namespaces">>, <<"/apis/apps/v1/statefulsets?limit=500">>},
        {<<"jobs">>, <<"all-namespaces">>, <<"/apis/batch/v1/jobs?limit=500">>},
        {<<"cronjobs">>, <<"all-namespaces">>, <<"/apis/batch/v1/cronjobs?limit=500">>},
        {<<"ingresses">>, <<"all-namespaces">>, <<"/apis/networking.k8s.io/v1/ingresses?limit=500">>},
        {<<"networkpolicies">>, <<"all-namespaces">>, <<"/apis/networking.k8s.io/v1/networkpolicies?limit=500">>},
        {<<"horizontalpodautoscalers">>, <<"all-namespaces">>, <<"/apis/autoscaling/v2/horizontalpodautoscalers?limit=500">>},
        {<<"storageclasses">>, <<"cluster">>, <<"/apis/storage.k8s.io/v1/storageclasses?limit=500">>},
        {<<"customresourcedefinitions">>, <<"cluster">>, <<"/apis/apiextensions.k8s.io/v1/customresourcedefinitions?limit=500">>}
    ].

deployments_path() ->
    Path = env_bin("MCP_KUBERNETES_DEPLOYMENTS_PATH", ?DEFAULT_DEPLOYMENTS_PATH),
    case Path of
        <<"/apis/apps/v1/deployments", _/binary>> -> Path;
        _ -> ?DEFAULT_DEPLOYMENTS_PATH
    end.

resource_json(Base, Limit, Name, Scope, Path) ->
    Url = join_url(Base, Path),
    Result = kubernetes_get(Url, Limit),
    json_obj([
        {<<"source">>, <<"kubernetes-api">>},
        {<<"resource">>, Name},
        {<<"scope">>, Scope},
        {<<"url">>, gleam_mcp_redact:url_for_output(Url)},
        {<<"readOnly">>, true},
        {<<"metadataOnlyRequest">>, true},
        {<<"response">>, raw(Result)}
    ]).

resource_entry(Base, Limit, {Name, Scope, Path}) ->
    Url = join_url(Base, Path),
    Result = kubernetes_get(Url, Limit),
    raw(json_obj([
        {<<"name">>, Name},
        {<<"scope">>, Scope},
        {<<"path">>, Path},
        {<<"url">>, gleam_mcp_redact:url_for_output(Url)},
        {<<"response">>, raw(Result)}
    ])).

kubernetes_get(UrlBin, Limit) ->
    _ = application:ensure_all_started(inets),
    _ = application:ensure_all_started(ssl),
    Timeout = bounded_int("MCP_KUBERNETES_TIMEOUT_MS", ?DEFAULT_TIMEOUT_MS, 100, ?MAX_TIMEOUT_MS),
    Started = erlang:monotonic_time(millisecond),
    Url = binary_to_list(UrlBin),
    TokenPath = binary_to_list(env_bin("MCP_KUBERNETES_TOKEN_PATH", ?DEFAULT_TOKEN_PATH)),
    CaPath = binary_to_list(env_bin("MCP_KUBERNETES_CA_PATH", ?DEFAULT_CA_PATH)),
    case file:read_file(TokenPath) of
        {ok, Token0} ->
            Token = binary:replace(Token0, <<"\n">>, <<>>, [global]),
            Headers = [
                {"accept", "application/json;as=PartialObjectMetadataList;g=meta.k8s.io;v=v1"},
                {"authorization", binary_to_list(<<"Bearer ", Token/binary>>)}
            ],
            HttpOptions = [
                {timeout, Timeout},
                {connect_timeout, Timeout},
                %% Never follow redirects: httpc re-sends headers (including this
                %% Bearer token) on a 3xx and does not strip them cross-host, so
                %% a redirect must not be able to retarget a token-bearing call.
                {autoredirect, false},
                {ssl, [{verify, verify_peer}, {cacertfile, CaPath}]}
            ],
            Options = [{body_format, binary}],
            case catch httpc:request(get, {Url, Headers}, HttpOptions, Options) of
                {ok, {{_, Status, Reason}, _RespHeaders, Body0}} ->
                    BodyOriginal = to_binary(Body0),
                    Body = gleam_mcp_redact:sample(BodyOriginal, Limit),
                    Truncated = byte_size(BodyOriginal) > Limit,
                    json_obj([
                        {<<"ok">>, Status >= 200 andalso Status < 400},
                        {<<"status">>, Status},
                        {<<"reason">>, to_binary(Reason)},
                        {<<"durationMs">>, elapsed_ms(Started)},
                        {<<"truncated">>, Truncated},
                        {<<"sample">>, Body}
                    ]);
                {error, Reason} ->
                    error_result(Reason, Started);
                {'EXIT', Reason} ->
                    error_result(Reason, Started)
            end;
        {error, Reason} ->
            json_obj([
                {<<"ok">>, false},
                {<<"durationMs">>, elapsed_ms(Started)},
                {<<"error">>, to_binary(io_lib:format("failed to read service account token: ~p", [Reason]))}
            ])
    end.

error_result(Reason, Started) ->
    json_obj([
        {<<"ok">>, false},
        {<<"durationMs">>, elapsed_ms(Started)},
        {<<"error">>, to_binary(io_lib:format("~p", [Reason]))}
    ]).

elapsed_ms(Started) ->
    erlang:monotonic_time(millisecond) - Started.

join_url(Base0, Path0) ->
    Base = trim_trailing_slash(Base0),
    Path =
        case Path0 of
            <<"/", _/binary>> -> Path0;
            _ -> <<"/", Path0/binary>>
        end,
    <<Base/binary, Path/binary>>.

trim_trailing_slash(<<>>) ->
    <<>>;
trim_trailing_slash(Bin) ->
    Size = byte_size(Bin),
    case binary:part(Bin, Size - 1, 1) of
        <<"/">> -> binary:part(Bin, 0, Size - 1);
        _ -> Bin
    end.

env_bin(Name, Default) ->
    dd_cli_config_client_ffi:getenv(Name, Default).

bounded_int(Name, Default, Min, Max) ->
    gleam_mcp_redact:bounded_int(Name, Default, Min, Max).

json_obj(Pairs) ->
    <<"{", (join([json_pair(K, V) || {K, V} <- Pairs], <<",">>))/binary, "}">>.

json_pair(Key, Value) ->
    <<(json_string(Key))/binary, ":", (json_value(Value))/binary>>.

json_value(Value) when is_binary(Value) -> json_string(Value);
json_value({raw, Value}) -> Value;
json_value({array, Values}) ->
    <<"[", (join([json_value(Value) || Value <- Values], <<",">>))/binary, "]">>;
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
