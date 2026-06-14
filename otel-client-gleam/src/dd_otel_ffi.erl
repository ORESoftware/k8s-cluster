%%% OpenTelemetry FFI for Gleam/Mist deployments.
%%%
%%% Explicit instrumentation only. This module is the thin Erlang boundary
%%% between Gleam (`dd_otel_client`) and the BEAM OpenTelemetry hex packages
%%% (`opentelemetry_api`, `opentelemetry`, `opentelemetry_exporter`). It does
%%% two jobs:
%%%
%%%   1. init/1 — configure + start the SDK and the OTLP exporter ONCE, before
%%%      the Mist supervisor comes up. The exporter reads OS env
%%%      `OTEL_EXPORTER_OTLP_ENDPOINT` / `OTEL_EXPORTER_OTLP_PROTOCOL`; we
%%%      default those to the in-cluster collector + http/protobuf if unset.
%%%      `service.name` is taken from the arg (also exported as
%%%      `OTEL_SERVICE_NAME`) and stamped onto the resource via the
%%%      `opentelemetry` application env, which is read when the SDK boots.
%%%
%%%   2. start_server_span/3 + set_status_code/2 + end_request_span/1 — the
%%%      per-request span lifecycle the Gleam Mist middleware drives. Inbound
%%%      W3C `traceparent` is decoded into the parent context so a span here
%%%      links to its caller.
%%%
%%% Every request-path function is wrapped in a catch so a missing/half-started
%%% SDK can never break request handling: the server keeps serving, just
%%% untraced.

-module(dd_otel_ffi).

-export([
    init/1,
    start_server_span/3,
    set_status_code/2,
    end_request_span/1
]).

-include_lib("opentelemetry_api/include/opentelemetry.hrl").
-include_lib("opentelemetry_api/include/otel_tracer.hrl").

-define(DEFAULT_ENDPOINT,
    "http://dd-otel-collector.observability.svc.cluster.local:4318").
-define(TRACER_NAME, dd_otel_client).

%% ---------------------------------------------------------------------------
%% init/1 — start provider + OTLP exporter, configure resource service.name.
%% Returns `nil` (Gleam Nil). Idempotent: safe to call more than once.
%% ---------------------------------------------------------------------------
init(ServiceName) ->
    ServiceBin = to_binary(ServiceName),
    try
        ensure_env("OTEL_EXPORTER_OTLP_ENDPOINT", ?DEFAULT_ENDPOINT),
        ensure_env("OTEL_EXPORTER_OTLP_PROTOCOL", "http_protobuf"),
        ensure_env("OTEL_SERVICE_NAME", binary_to_list(ServiceBin)),

        %% Resource service.name via the opentelemetry app env. Read by
        %% otel_resource_app_env when the SDK boots, so it MUST be set before
        %% the application starts. Merge rather than clobber any operator value.
        Existing = application:get_env(opentelemetry, resource, #{}),
        Resource = merge_service_name(Existing, ServiceBin),
        application:set_env(opentelemetry, resource, Resource),

        %% Batch span processor + OTLP exporter. These are the SDK defaults
        %% too, but pinning them makes the intent explicit and independent of
        %% any inherited release config.
        application:set_env(opentelemetry, span_processor, batch),
        application:set_env(opentelemetry, traces_exporter, otlp),

        %% Exporter first (the SDK's batch processor links to it on boot),
        %% then the SDK itself.
        {ok, _} = ensure_started(opentelemetry_exporter),
        {ok, _} = ensure_started(opentelemetry),

        io:format(
            "[otel] initialized service.name=~ts endpoint=~ts protocol=~ts~n",
            [ServiceBin, getenv_or("OTEL_EXPORTER_OTLP_ENDPOINT", ?DEFAULT_ENDPOINT),
             getenv_or("OTEL_EXPORTER_OTLP_PROTOCOL", "http_protobuf")]
        ),
        nil
    catch
        Class:Reason:Stack ->
            io:format(
                "[otel] init failed (continuing without tracing): ~p:~p~n~p~n",
                [Class, Reason, Stack]
            ),
            nil
    end.

%% ---------------------------------------------------------------------------
%% start_server_span(Method, Path, Traceparent) -> Span
%%
%%   Method      :: binary()  (e.g. <<"GET">>)
%%   Path        :: binary()  (e.g. <<"/healthz">>)
%%   Traceparent :: {ok, binary()} | {error, nil}   (Gleam Result(String, Nil))
%%
%% Starts a SERVER span named "{METHOD} {PATH}", with the inbound W3C
%% traceparent (if any) as the parent. The returned opaque `Span` is threaded
%% back into set_status_code/2 and end_request_span/1. On any failure we hand
%% back a `no_span` sentinel so the caller's later calls are no-ops.
%% ---------------------------------------------------------------------------
start_server_span(Method, Path, Traceparent) ->
    try
        ParentToken = attach_parent(Traceparent),
        Name = span_name(Method, Path),
        Tracer = opentelemetry:get_tracer(?TRACER_NAME),
        SpanCtx = otel_tracer:start_span(Tracer, Name, #{kind => ?SPAN_KIND_SERVER}),
        _ = otel_tracer:set_current_span(SpanCtx),
        otel_span:set_attributes(SpanCtx, [
            {<<"http.request.method">>, Method},
            {<<"url.path">>, Path}
        ]),
        {span, SpanCtx, ParentToken}
    catch
        _:_ -> no_span
    end.

%% ---------------------------------------------------------------------------
%% set_status_code(Span, Status) -> nil
%% Stamps http.response.status_code and, for >= 500, marks the span error.
%% ---------------------------------------------------------------------------
set_status_code(no_span, _Status) ->
    nil;
set_status_code({span, SpanCtx, _Token}, Status) ->
    try
        otel_span:set_attribute(SpanCtx, <<"http.response.status_code">>, Status),
        case Status >= 500 of
            true -> otel_span:set_status(SpanCtx, ?OTEL_STATUS_ERROR, <<>>);
            false -> ok
        end,
        nil
    catch
        _:_ -> nil
    end.

%% ---------------------------------------------------------------------------
%% end_request_span(Span) -> nil
%% Ends the span and detaches the parent context token we attached at start.
%% ---------------------------------------------------------------------------
end_request_span(no_span) ->
    nil;
end_request_span({span, SpanCtx, Token}) ->
    try
        _ = otel_span:end_span(SpanCtx),
        detach_parent(Token),
        nil
    catch
        _:_ -> nil
    end.

%% ---------------------------------------------------------------------------
%% Internal
%% ---------------------------------------------------------------------------

%% Build the parent context from an inbound traceparent header, attach it as
%% the current context, and return a token to detach later. `undefined` means
%% no parent / nothing to detach.
attach_parent({ok, Header}) when is_binary(Header), Header =/= <<>> ->
    Carrier = [{<<"traceparent">>, Header}],
    Ctx = otel_propagator_trace_context:extract(
        otel_ctx:get_current(),
        Carrier,
        fun(_) -> [<<"traceparent">>] end,
        fun carrier_get/2,
        []
    ),
    otel_ctx:attach(Ctx);
attach_parent(_) ->
    undefined.

detach_parent(undefined) ->
    ok;
detach_parent(Token) ->
    catch otel_ctx:detach(Token),
    ok.

carrier_get(Key, Carrier) ->
    case lists:keyfind(Key, 1, Carrier) of
        {Key, Value} -> Value;
        false -> undefined
    end.

span_name(Method, Path) ->
    <<(to_binary(Method))/binary, " ", (to_binary(Path))/binary>>.

merge_service_name(Existing, ServiceBin) when is_map(Existing) ->
    Service0 = maps:get(service, Existing, #{}),
    Service = case is_map(Service0) of
        true -> Service0#{name => ServiceBin};
        false -> #{name => ServiceBin}
    end,
    Existing#{service => Service};
merge_service_name(_Existing, ServiceBin) ->
    #{service => #{name => ServiceBin}}.

%% Only set an OS env var when it is unset/empty, so operator-provided values
%% (e.g. a real collector endpoint injected by k8s) always win.
ensure_env(Name, Default) ->
    case os:getenv(Name) of
        false -> os:putenv(Name, Default);
        "" -> os:putenv(Name, Default);
        _ -> true
    end.

getenv_or(Name, Default) ->
    case os:getenv(Name) of
        false -> Default;
        "" -> Default;
        Value -> Value
    end.

ensure_started(App) ->
    case application:ensure_all_started(App) of
        {ok, Started} -> {ok, Started};
        {error, {already_started, _}} -> {ok, []};
        {error, Reason} ->
            io:format("[otel] could not start ~p: ~p~n", [App, Reason]),
            {ok, []}
    end.

to_binary(B) when is_binary(B) -> B;
to_binary(L) when is_list(L) -> unicode:characters_to_binary(L);
to_binary(A) when is_atom(A) -> atom_to_binary(A, utf8);
to_binary(I) when is_integer(I) -> integer_to_binary(I).
