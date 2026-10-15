#!/usr/bin/env escript
%%! -pa build

main(_) ->
    check_lock_request(),
    check_composite_request(),
    check_response_types(),
    io:format("[test-erlang] all protocol tests passed~n").

check_lock_request() ->
    Frame = network_mutex_protocol:lock_request_single("u-1", "k1", 4000, 1, false),
    assert(contains(Frame, "\"type\":\"lock\""), "lock type"),
    assert(contains(Frame, "\"key\":\"k1\""), "single key"),
    assert(contains(Frame, "\"ttl\":4000"), "ttl"),
    assert(contains(Frame, "\"max\":1"), "max"),
    assert(contains(Frame, "\"wait\":false"), "wait false"),
    assert(lists:suffix("\n", Frame), "newline").

check_composite_request() ->
    Frame = network_mutex_protocol:lock_request_composite("u-2", ["c", "a", "b"], 0, true),
    assert(contains(Frame, "\"keys\":[\"c\",\"a\",\"b\"]"), "keys preserved"),
    assert(contains(Frame, "\"wait\":true"), "wait true"),
    try network_mutex_protocol:lock_request_composite("u", ["a", "b", "c", "d", "e", "f"], 0, undefined) of
        _ -> fail("oversize composite should throw")
    catch
        error:{bad_composite_key_count, 6} -> ok
    end.

check_response_types() ->
    composite_lock = network_mutex_protocol:response_type_from_wire("compositeLock"),
    register_read_result = network_mutex_protocol:response_type_from_wire("registerReadResult"),
    ok = network_mutex_protocol:response_type_from_wire("ok"),
    unknown = network_mutex_protocol:response_type_from_wire("totallyBogus").

contains(Haystack, Needle) ->
    string:find(Haystack, Needle) =/= nomatch.

assert(true, _) -> ok;
assert(false, Name) -> fail(Name).

fail(Name) ->
    io:format(standard_error, "FAIL: ~s~n", [Name]),
    halt(1).

