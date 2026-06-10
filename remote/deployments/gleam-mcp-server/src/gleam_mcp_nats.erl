%%% Small FFI helpers for the NATS transport: the local BEAM node name
%%% (used as the `Source-Node` header so the server drops its own echoes)
%%% and a millisecond wall clock for event timestamps.
-module(gleam_mcp_nats).

-export([self_node_binary/0, now_ms/0]).

self_node_binary() ->
    atom_to_binary(node(), utf8).

now_ms() ->
    os:system_time(millisecond).
