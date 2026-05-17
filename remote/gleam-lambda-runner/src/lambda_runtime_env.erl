-module(lambda_runtime_env).

-export([getenv/1]).

getenv(Name) when is_binary(Name) ->
    getenv(binary_to_list(Name));
getenv(Name) when is_list(Name) ->
    case os:getenv(Name) of
        false -> <<>>;
        "" -> <<>>;
        Value -> unicode:characters_to_binary(Value)
    end.
