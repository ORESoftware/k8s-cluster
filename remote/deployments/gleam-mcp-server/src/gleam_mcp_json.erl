-module(gleam_mcp_json).

-export([method/1, request_id/1, tool_name/1]).

method(Body0) ->
    case decode_request(Body0) of
        {ok, #{<<"jsonrpc">> := <<"2.0">>, <<"method">> := Method}}
            when is_binary(Method), byte_size(Method) > 0 ->
            Method;
        {ok, _Other} ->
            <<"invalid_request">>;
        {error, _Reason} ->
            <<"parse_error">>
    end.

request_id(Body0) ->
    case decode_request(Body0) of
        {ok, #{<<"id">> := Id}} -> id_json(Id);
        _ -> <<"null">>
    end.

tool_name(Body0) ->
    case decode_request(Body0) of
        {ok, #{<<"params">> := #{<<"name">> := Name}}} when is_binary(Name) ->
            Name;
        _ ->
            <<"unknown">>
    end.

decode_request(Body0) ->
    Body = to_binary(Body0),
    try json:decode(Body) of
        Map when is_map(Map) -> {ok, Map};
        _Other -> {error, not_object}
    catch
        Class:Reason -> {error, {Class, Reason}}
    end.

id_json(Id) when is_binary(Id) ->
    json_string(Id);
id_json(Id) when is_integer(Id) ->
    integer_to_binary(Id);
id_json(Id) when is_float(Id) ->
    unicode:characters_to_binary(io_lib:format("~p", [Id]));
id_json(null) ->
    <<"null">>;
id_json(_Other) ->
    <<"null">>.

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

to_binary(Value) when is_binary(Value) ->
    Value;
to_binary(Value) when is_list(Value) ->
    unicode:characters_to_binary(Value);
to_binary(Value) ->
    unicode:characters_to_binary(io_lib:format("~p", [Value])).
