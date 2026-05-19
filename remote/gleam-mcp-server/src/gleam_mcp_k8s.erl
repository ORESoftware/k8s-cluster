-module(gleam_mcp_k8s).

-export([deployments_json/0]).

-define(DEFAULT_API_URL, <<"https://kubernetes.default.svc">>).
-define(DEFAULT_DEPLOYMENTS_PATH, <<"/apis/apps/v1/deployments?limit=500">>).
-define(DEFAULT_TOKEN_PATH, <<"/var/run/secrets/kubernetes.io/serviceaccount/token">>).
-define(DEFAULT_CA_PATH, <<"/var/run/secrets/kubernetes.io/serviceaccount/ca.crt">>).
-define(DEFAULT_TIMEOUT_MS, 1500).
-define(DEFAULT_BODY_LIMIT_BYTES, 262144).

deployments_json() ->
    Url = join_url(
        env_bin("MCP_KUBERNETES_API_URL", ?DEFAULT_API_URL),
        env_bin("MCP_KUBERNETES_DEPLOYMENTS_PATH", ?DEFAULT_DEPLOYMENTS_PATH)
    ),
    Limit = env_pos_int("MCP_KUBERNETES_BODY_LIMIT_BYTES", ?DEFAULT_BODY_LIMIT_BYTES),
    Result = kubernetes_get(Url, Limit),
    json_obj([
        {<<"source">>, <<"kubernetes-api">>},
        {<<"scope">>, <<"all-namespaces deployments">>},
        {<<"url">>, Url},
        {<<"readOnly">>, true},
        {<<"response">>, raw(Result)}
    ]).

kubernetes_get(UrlBin, Limit) ->
    _ = application:ensure_all_started(inets),
    _ = application:ensure_all_started(ssl),
    Timeout = env_pos_int("MCP_KUBERNETES_TIMEOUT_MS", ?DEFAULT_TIMEOUT_MS),
    Started = erlang:monotonic_time(millisecond),
    Url = binary_to_list(UrlBin),
    TokenPath = binary_to_list(env_bin("MCP_KUBERNETES_TOKEN_PATH", ?DEFAULT_TOKEN_PATH)),
    CaPath = binary_to_list(env_bin("MCP_KUBERNETES_CA_PATH", ?DEFAULT_CA_PATH)),
    case file:read_file(TokenPath) of
        {ok, Token0} ->
            Token = binary:replace(Token0, <<"\n">>, <<>>, [global]),
            Headers = [
                {"accept", "application/json;as=PartialObjectMetadataList;g=meta.k8s.io;v=v1, application/json"},
                {"authorization", binary_to_list(<<"Bearer ", Token/binary>>)}
            ],
            HttpOptions = [
                {timeout, Timeout},
                {connect_timeout, Timeout},
                {ssl, [{verify, verify_peer}, {cacertfile, CaPath}]}
            ],
            Options = [{body_format, binary}],
            case catch httpc:request(get, {Url, Headers}, HttpOptions, Options) of
                {ok, {{_, Status, Reason}, _RespHeaders, Body0}} ->
                    BodyOriginal = to_binary(Body0),
                    Body = clip(BodyOriginal, Limit),
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
    case os:getenv(Name) of
        false -> Default;
        "" -> Default;
        Value -> unicode:characters_to_binary(Value)
    end.

env_pos_int(Name, Default) ->
    case os:getenv(Name) of
        false -> Default;
        "" -> Default;
        Raw ->
            case string:to_integer(Raw) of
                {Value, _} when Value > 0 -> Value;
                _ -> Default
            end
    end.

clip(Bin, Limit) when byte_size(Bin) =< Limit -> Bin;
clip(Bin, Limit) when Limit > 32 ->
    Prefix = binary:part(Bin, 0, Limit),
    <<Prefix/binary, "\n... clipped ...">>;
clip(Bin, Limit) ->
    binary:part(Bin, 0, Limit).

json_obj(Pairs) ->
    <<"{", (join([json_pair(K, V) || {K, V} <- Pairs], <<",">>))/binary, "}">>.

json_pair(Key, Value) ->
    <<(json_string(Key))/binary, ":", (json_value(Value))/binary>>.

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
