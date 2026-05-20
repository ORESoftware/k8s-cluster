-module(gleam_mcp_json).

-export([request_id/1]).

request_id(Body0) ->
    Body = to_binary(Body0),
    Pattern = <<"\"id\"\\s*:\\s*(\"(?:[^\"\\\\]|\\\\.)*\"|-?(?:0|[1-9][0-9]*)(?:\\.[0-9]+)?(?:[eE][+-]?[0-9]+)?|null)">>,
    case re:run(Body, Pattern, [{capture, [1], binary}, unicode]) of
        {match, [Id]} -> Id;
        _ -> <<"1">>
    end.

to_binary(Value) when is_binary(Value) ->
    Value;
to_binary(Value) when is_list(Value) ->
    unicode:characters_to_binary(Value);
to_binary(Value) ->
    unicode:characters_to_binary(io_lib:format("~p", [Value])).
