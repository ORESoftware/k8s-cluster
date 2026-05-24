%% FFI helpers for the Gleam mutex load tester.
%%
%% Kept tiny on purpose — most of these are one-liners that wrap an
%% Erlang/OTP standard call to give Gleam a strongly-typed `String ->
%% Int` (or similar) signature without forcing every callsite to deal
%% with `os:getenv/1` returning the atom `false` for missing keys.

-module(lock_loadtest_gleam_env_ffi).

-export([getenv/2, system_time_micro/0, sleep_ms/1, uuid_v4/0]).

%% Read an env var. Erlang's `os:getenv/1` returns the atom `false`
%% when the variable is unset, which is awkward to thread through
%% Gleam's `Result`. Returning the supplied fallback on miss lets the
%% Gleam side stay branch-free.
getenv(Name, Fallback) ->
    case os:getenv(binary_to_list(Name)) of
        false -> Fallback;
        Value -> list_to_binary(Value)
    end.

%% Microsecond-precision wall clock. We use this for both the
%% per-acquire latency stopwatch (nanoseconds is overkill given the
%% live-mutex protocol's microsecond-level latencies in practice) and
%% the bench-window deadline arithmetic.
system_time_micro() ->
    erlang:system_time(microsecond).

%% Plain BEAM sleep. `gleam_erlang/process` exposes a `sleep/1` but
%% pulling the whole process module into the loop file just for
%% `sleep` adds noise; the FFI is one line.
sleep_ms(MS) ->
    timer:sleep(MS),
    nil.

%% Hex-encoded v4 UUID. We avoid the `uuid` external dep (not
%% available out of the box on `rebar3`/Gleam target) and instead
%% format `crypto:strong_rand_bytes/1` ourselves. Length and dash
%% positions match RFC 4122 §4.4 so logs interleave cleanly with the
%% Rust load tester's `Uuid::new_v4`.
uuid_v4() ->
    <<A:32, B:16, _:4, C:12, _:2, D:14, E:48>> = crypto:strong_rand_bytes(16),
    Hex = io_lib:format(
        "~8.16.0b-~4.16.0b-4~3.16.0b-~4.16.0b-~12.16.0b",
        [A, B, C, 16#8000 bor D, E]
    ),
    list_to_binary(lists:flatten(Hex)).
