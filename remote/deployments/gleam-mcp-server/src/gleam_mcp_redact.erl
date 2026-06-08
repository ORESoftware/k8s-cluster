-module(gleam_mcp_redact).

-export([bounded_int/4, sample/2, safe_base_url/2, url_for_output/1]).

-define(REDACTED, <<"<redacted>">>).

bounded_int(Name, Default, Min, Max) ->
    Raw = dd_cli_config_client_ffi:getenv(Name, <<>>),
    case string:to_integer(binary_to_list(Raw)) of
        {Value, _} when Value >= Min, Value =< Max -> Value;
        _ -> Default
    end.

safe_base_url(Name, Default) ->
    Raw = dd_cli_config_client_ffi:getenv(Name, Default),
    case allow_external_urls() orelse allowed_base_url(Raw) of
        true -> Raw;
        false -> Default
    end.

url_for_output(Url0) ->
    Url = to_binary(Url0),
    try uri_string:parse(binary_to_list(Url)) of
        Map0 when is_map(Map0) ->
            Map1 =
                case maps:is_key(userinfo, Map0) of
                    true -> maps:put(userinfo, "redacted", Map0);
                    false -> Map0
                end,
            Map2 =
                case maps:is_key(query, Map1) of
                    true -> maps:put(query, "redacted=1", Map1);
                    false -> Map1
                end,
            to_binary(uri_string:recompose(maps:remove(fragment, Map2)));
        _ ->
            redact_text(Url)
    catch
        _:_ -> redact_text(Url)
    end.

sample(Body0, Limit) ->
    Body = to_binary(Body0),
    Redacted = redact_body(Body),
    clip(Redacted, Limit).

redact_body(Body) ->
    try json:decode(Body) of
        Value -> to_binary(json:encode(redact_json(Value)))
    catch
        _:_ -> redact_text(Body)
    end.

redact_json(Map) when is_map(Map) ->
    maps:fold(
        fun(Key, Value, Acc) ->
            RedactedValue =
                case secret_like_key(Key) of
                    true -> ?REDACTED;
                    false -> redact_json(Value)
                end,
            maps:put(Key, RedactedValue, Acc)
        end,
        #{},
        Map
    );
redact_json(List) when is_list(List) ->
    [redact_json(Value) || Value <- List];
redact_json(Value) ->
    Value.

redact_text(Bin) ->
    join([redact_line(Line) || Line <- binary:split(Bin, <<"\n">>, [global])], <<"\n">>).

redact_line(Line) ->
    case line_has_secret_key(Line) of
        false ->
            Line;
        true ->
            case first_delimiter(Line) of
                {ok, Index} ->
                    <<(binary:part(Line, 0, Index + 1))/binary, ?REDACTED/binary>>;
                error ->
                    ?REDACTED
            end
    end.

line_has_secret_key(Line) ->
    Tokens = re:split(Line, <<"[^A-Za-z0-9_-]+">>, [trim, {return, binary}]),
    lists:any(fun secret_like_key/1, Tokens).

secret_like_key(Key0) ->
    Key = string:lowercase(binary_to_list(to_binary(Key0))),
    Normalized = [C || C <- Key, C =/= $-, C =/= $_, C =/= $.],
    contains(Key, "authorization")
        orelse contains(Key, "cookie")
        orelse contains(Key, "credential")
        orelse contains(Key, "password")
        orelse contains(Key, "secret")
        orelse contains(Key, "session")
        orelse contains(Key, "token")
        orelse contains(Normalized, "apikey")
        orelse contains(Normalized, "accesskey")
        orelse contains(Normalized, "privatekey")
        orelse contains(Normalized, "clientsecret").

contains(Haystack, Needle) ->
    string:find(Haystack, Needle) =/= nomatch.

first_delimiter(Line) ->
    case {binary:match(Line, <<"=">>), binary:match(Line, <<":">>)} of
        {{Eq, _}, {Colon, _}} -> {ok, min(Eq, Colon)};
        {{Eq, _}, nomatch} -> {ok, Eq};
        {nomatch, {Colon, _}} -> {ok, Colon};
        {nomatch, nomatch} -> error
    end.

allowed_base_url(Url0) ->
    Url = to_binary(Url0),
    try uri_string:parse(binary_to_list(Url)) of
        #{scheme := Scheme0, host := Host0} = Map ->
            Scheme = string:lowercase(Scheme0),
            Host = string:lowercase(trim_trailing_dot(Host0)),
            SchemeOk = Scheme =:= "http" orelse Scheme =:= "https",
            BaseOnly = not maps:is_key(userinfo, Map)
                andalso not maps:is_key(query, Map)
                andalso not maps:is_key(fragment, Map)
                andalso base_path_ok(maps:get(path, Map, "")),
            SchemeOk andalso BaseOnly andalso allowed_host(Host);
        _ ->
            false
    catch
        _:_ -> false
    end.

base_path_ok("") -> true;
base_path_ok("/") -> true;
base_path_ok(_) -> false.

allowed_host("localhost") -> true;
allowed_host("127.0.0.1") -> true;
allowed_host("::1") -> true;
allowed_host("[::1]") -> true;
allowed_host("kubernetes.default.svc") -> true;
allowed_host(Host) ->
    has_suffix(Host, ".svc") orelse has_suffix(Host, ".svc.cluster.local").

has_suffix(Value, Suffix) ->
    ValueLen = length(Value),
    SuffixLen = length(Suffix),
    ValueLen >= SuffixLen andalso lists:suffix(Suffix, Value).

allow_external_urls() ->
    case dd_cli_config_client_ffi:getenv("MCP_ALLOW_EXTERNAL_URLS", <<>>) of
        <<"1">> -> true;
        <<"true">> -> true;
        <<"TRUE">> -> true;
        <<"yes">> -> true;
        <<"YES">> -> true;
        _ -> false
    end.

trim_trailing_dot([]) ->
    [];
trim_trailing_dot(Value) ->
    case lists:last(Value) of
        $. -> lists:sublist(Value, length(Value) - 1);
        _ -> Value
    end.

clip(Bin, Limit) when byte_size(Bin) =< Limit -> Bin;
clip(Bin, Limit) when Limit > 32 ->
    Prefix = binary:part(Bin, 0, Limit),
    <<Prefix/binary, "\n... clipped ...">>;
clip(Bin, Limit) ->
    binary:part(Bin, 0, Limit).

join([], _Sep) -> <<>>;
join([One], _Sep) -> One;
join([First | Rest], Sep) ->
    <<First/binary, Sep/binary, (join(Rest, Sep))/binary>>.

to_binary(Value) when is_binary(Value) -> Value;
to_binary(Value) when is_list(Value) -> unicode:characters_to_binary(Value);
to_binary(Value) -> unicode:characters_to_binary(io_lib:format("~p", [Value])).
