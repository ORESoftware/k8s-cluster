%%% =====================================================================
%%% JSON-RPC request parsing for the Gleam MCP server.
%%%
%%% Previously this module only regex-extracted the `id`, and the Gleam
%%% layer routed by substring-matching the raw body for `"tools/call"`,
%%% `"kubernetes_inventory"`, etc. That is fragile and unsafe: any request
%%% whose *arguments* contained one of those literals (a string param, a
%%% nested object) would misroute, and the `id` regex could latch onto an
%%% `id` nested inside `params`. The moment a tool takes arguments, the
%%% substring router is exploitable.
%%%
%%% `parse_request/1` does a single real JSON parse (OTP `json`, present
%%% since OTP 27 — the deploy image is erlang-alpine OTP 27+) and returns
%%% the *top-level* JSON-RPC fields the router actually needs:
%%%
%%%   {Status, Method, IdJson, Tool}
%%%
%%%   Status :: <<"request">>         — object with a method and an id
%%%           | <<"notification">>    — object with a method but no id
%%%           | <<"invalid_request">> — valid JSON but not a JSON-RPC object
%%%                                      (top-level array/batch, bare scalar,
%%%                                      or missing/invalid method)
%%%           | <<"parse_error">>     — body is not valid JSON
%%%   Method :: binary()  — the verbatim top-level "method" (<<>> if none)
%%%   IdJson :: binary()  — the "id" re-encoded as JSON verbatim (number,
%%%                         string, or null). <<"null">> when absent/unknown,
%%%                         which is exactly what JSON-RPC requires for the
%%%                         id of an error response to an unidentifiable
%%%                         request.
%%%   Tool   :: binary()  — params.name for tools/call (<<>> if absent)
%%%
%%% MCP's Streamable HTTP transport (protocol 2025-11-25) removed JSON-RPC
%%% batching, so a top-level array is reported as invalid_request rather
%%% than fanned out.
%%%
%%% If the OTP `json` module is somehow unavailable (OTP < 27), we fall
%%% back to the historical substring/regex extraction so behaviour
%%% degrades to the old best-effort routing instead of failing outright.
%%% =====================================================================
-module(gleam_mcp_json).

-export([parse_request/1, request_id/1]).

%%% ---------------------------------------------------------------------
%%% Public API
%%% ---------------------------------------------------------------------

parse_request(Body0) ->
    Body = to_binary(Body0),
    case decode_json(Body) of
        {ok, Decoded} -> from_decoded(Decoded);
        no_json -> legacy_parse(Body);
        error -> {<<"parse_error">>, <<>>, <<"null">>, <<>>}
    end.

%% Retained for backward compatibility with any caller still asking only
%% for the id. New code should use parse_request/1.
request_id(Body0) ->
    {_Status, _Method, IdJson, _Tool} = parse_request(Body0),
    IdJson.

%%% ---------------------------------------------------------------------
%%% Decode
%%% ---------------------------------------------------------------------

decode_json(Body) ->
    try
        {ok, json:decode(Body)}
    catch
        error:undef -> no_json;   %% OTP json module not available
        _:_ -> error              %% malformed JSON
    end.

from_decoded(Decoded) when is_list(Decoded) ->
    %% Top-level JSON array => JSON-RPC batch, unsupported in MCP 2025-11-25.
    {<<"invalid_request">>, <<>>, <<"null">>, <<>>};
from_decoded(Decoded) when is_map(Decoded) ->
    Method = bin_field(Decoded, <<"method">>),
    HasId = maps:is_key(<<"id">>, Decoded),
    IdJson =
        case HasId of
            true -> encode_id(maps:get(<<"id">>, Decoded));
            false -> <<"null">>
        end,
    Tool = tool_name(maps:get(<<"params">>, Decoded, undefined)),
    case Method of
        <<>> ->
            %% A JSON object with no usable method is not a valid request.
            {<<"invalid_request">>, <<>>, IdJson, <<>>};
        _ when HasId -> {<<"request">>, Method, IdJson, Tool};
        _ -> {<<"notification">>, Method, <<"null">>, Tool}
    end;
from_decoded(_Other) ->
    %% Valid JSON but a bare scalar (number/string/bool/null) — not a
    %% JSON-RPC request.
    {<<"invalid_request">>, <<>>, <<"null">>, <<>>}.

tool_name(Params) when is_map(Params) -> bin_field(Params, <<"name">>);
tool_name(_) -> <<>>.

bin_field(Map, Key) ->
    case maps:get(Key, Map, undefined) of
        V when is_binary(V) -> V;
        _ -> <<>>
    end.

%% Re-encode the decoded id verbatim so the response echoes the exact
%% JSON value the client sent (number, string, or null).
encode_id(Id) ->
    try
        iolist_to_binary(json:encode(Id))
    catch
        _:_ -> <<"null">>
    end.

%%% ---------------------------------------------------------------------
%%% Legacy fallback (only when OTP `json` is unavailable)
%%% ---------------------------------------------------------------------

legacy_parse(Body) ->
    Method = legacy_method(Body),
    IdJson = legacy_id(Body),
    Tool = legacy_tool(Body),
    case Method of
        <<>> -> {<<"invalid_request">>, <<>>, IdJson, <<>>};
        _ -> {<<"request">>, Method, IdJson, Tool}
    end.

legacy_method(Body) ->
    first_contained(Body, [
        <<"tools/call">>,
        <<"tools/list">>,
        <<"initialize">>,
        <<"notifications/initialized">>,
        <<"ping">>
    ]).

legacy_tool(Body) ->
    first_contained(Body, [
        <<"kubernetes_inventory">>,
        <<"kubernetes_deployments">>,
        <<"human_access_policy">>,
        <<"telemetry_summary">>,
        <<"observability_health">>,
        <<"prometheus_up">>,
        <<"loki_labels">>,
        <<"grafana_inventory">>,
        <<"nats_metrics">>,
        <<"trace_backends">>,
        <<"telemetry_targets">>,
        <<"service_directory">>,
        <<"cluster_status">>
    ]).

first_contained(_Body, []) -> <<>>;
first_contained(Body, [Needle | Rest]) ->
    Quoted = <<"\"", Needle/binary, "\"">>,
    case binary:match(Body, Quoted) of
        nomatch -> first_contained(Body, Rest);
        _ -> Needle
    end.

legacy_id(Body) ->
    Pattern =
        <<"\"id\"\\s*:\\s*(\"(?:[^\"\\\\]|\\\\.)*\"|-?(?:0|[1-9][0-9]*)(?:\\.[0-9]+)?(?:[eE][+-]?[0-9]+)?|null)">>,
    case re:run(Body, Pattern, [{capture, [1], binary}, unicode]) of
        {match, [Id]} -> Id;
        _ -> <<"null">>
    end.

%%% ---------------------------------------------------------------------
%%% Helpers
%%% ---------------------------------------------------------------------

to_binary(Value) when is_binary(Value) ->
    Value;
to_binary(Value) when is_list(Value) ->
    unicode:characters_to_binary(Value);
to_binary(Value) ->
    unicode:characters_to_binary(io_lib:format("~p", [Value])).
