-module(gleamlang_presence_server_ffi).

-export([stable_name/1, env/1, read_file_utf8/1]).

%% Build a process Name (which is just an Erlang atom internally) from a
%% known string. Unlike `gleam_erlang_ffi:new_name/1` this does NOT append
%% a unique suffix, so two BEAM nodes that both call
%% `stable_name(<<"presence_fanout_relay">>)` end up with the SAME atom and
%% therefore the SAME `Name(msg)` value.
stable_name(S) ->
    erlang:binary_to_atom(S, utf8).

%% Read an environment variable as a UTF-8 binary, returning `{ok, Value}`
%% or `{error, nil}`. The Gleam side decodes the result via a small wrapper.
env(Name) ->
    case os:getenv(binary_to_list(Name)) of
        false -> {error, nil};
        Value -> {ok, list_to_binary(Value)}
    end.

%% Read a small file (a few KB) into a UTF-8 binary. Used to read the
%% in-pod k8s service account token (~1 KB JWT). Returns `{ok, Body}` or
%% `{error, ReasonBinary}`.
read_file_utf8(Path) ->
    case file:read_file(Path) of
        {ok, Body} -> {ok, Body};
        {error, Reason} ->
            ReasonBin =
                list_to_binary(io_lib:format("~p", [Reason])),
            {error, ReasonBin}
    end.
