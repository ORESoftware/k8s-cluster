-module(gleam_mcp_runtime_env).

-export([getenv/1]).

getenv(Name) when is_binary(Name) ->
    getenv(binary_to_list(Name));
getenv(Name) when is_list(Name) ->
    dd_cli_config_client_ffi:getenv(Name, <<>>).
