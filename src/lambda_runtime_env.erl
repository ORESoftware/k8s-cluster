-module(lambda_runtime_env).

-export([getenv/1, putenv/2, secret_equals/2]).

%% Constant-time secret comparison for shared-secret header auth.
%%
%% `=:=' on binaries returns as soon as it finds a differing byte, which leaks
%% the length of the matching prefix to anyone who can time the request. Hash
%% both sides first so the compared binaries are always 32 bytes (crypto:hash_equals
%% raises on a length mismatch) and the comparison itself is fixed-time.
secret_equals(Provided, Expected) when is_binary(Provided), is_binary(Expected) ->
    crypto:hash_equals(
        crypto:hash(sha256, Provided),
        crypto:hash(sha256, Expected)
    );
secret_equals(Provided, Expected) when is_list(Provided) ->
    secret_equals(list_to_binary(Provided), Expected);
secret_equals(Provided, Expected) when is_list(Expected) ->
    secret_equals(Provided, list_to_binary(Expected));
secret_equals(_Provided, _Expected) ->
    false.

getenv(Name) when is_binary(Name) ->
    getenv(binary_to_list(Name));
getenv(Name) when is_list(Name) ->
    dd_cli_config_client_ffi:getenv(Name, <<>>).

putenv(Name, Value) when is_binary(Name) ->
    putenv(binary_to_list(Name), Value);
putenv(Name, Value) when is_binary(Value) ->
    putenv(Name, binary_to_list(Value));
putenv(Name, Value) when is_list(Name), is_list(Value) ->
    os:putenv(Name, Value).
