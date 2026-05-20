-module(lambda_runtime_env).

-export([getenv/1, putenv/2]).

getenv(Name) when is_binary(Name) ->
    getenv(binary_to_list(Name));
getenv(Name) when is_list(Name) ->
    case os:getenv(Name) of
        false -> <<>>;
        "" -> <<>>;
        Value -> unicode:characters_to_binary(Value)
    end.

putenv(Name, Value) when is_binary(Name) ->
    putenv(binary_to_list(Name), Value);
putenv(Name, Value) when is_binary(Value) ->
    putenv(Name, binary_to_list(Value));
putenv(Name, Value) when is_list(Name), is_list(Value) ->
    os:putenv(Name, Value).
