%% EUnit regression tests for the workflow engine's pure hardening helpers.
%% Run with: erl -pa <ebin dirs> -eval 'eunit:test(workflow_logic_test, [verbose]), halt().'
%% (These cover the truncation / backoff-overflow / retry-cap guards that prevent
%% un-persistable values and unbounded retries from fail-looping a run.)
-module(workflow_logic_test).
-include_lib("eunit/include/eunit.hrl").

clamp_text_keeps_short_test() ->
    ?assertEqual(<<"abc">>, workflow_store:clamp_text(<<"abc">>, 10)).

clamp_text_truncates_ascii_test() ->
    ?assertEqual(<<"abcd">>, workflow_store:clamp_text(<<"abcdefghij">>, 4)).

clamp_text_lands_on_utf8_boundary_test() ->
    %% "h" ++ é(2 bytes): clamping to 2 bytes would split é, so the partial
    %% trailing byte must be dropped, leaving valid UTF-8.
    Clamped = workflow_store:clamp_text(<<"h", 16#C3, 16#A9>>, 2),
    ?assert(byte_size(Clamped) =< 2),
    ?assertMatch(B when is_binary(B), unicode:characters_to_binary(Clamped, utf8, utf8)).

backoff_default_first_attempt_test() ->
    ?assertEqual(1000, workflow_engine:backoff_ms(#{}, 0)).

backoff_exponential_test() ->
    ?assertEqual(8000, workflow_engine:backoff_ms(#{}, 3)).

backoff_caps_at_max_test() ->
    ?assertEqual(60000, workflow_engine:backoff_ms(#{<<"maxBackoffMs">> => 60000}, 50)).

backoff_survives_huge_attempt_test() ->
    %% Must not raise on a pathological exponent.
    ?assertEqual(60000,
        workflow_engine:backoff_ms(#{<<"backoffFactor">> => 10.0, <<"maxBackoffMs">> => 60000}, 100000)).

max_attempts_default_test() ->
    ?assertEqual(3, workflow_engine:max_attempts(#{})).

max_attempts_caps_high_test() ->
    ?assertEqual(1000, workflow_engine:max_attempts(#{<<"maxAttempts">> => 1000000000})).

max_attempts_floors_low_test() ->
    ?assertEqual(1, workflow_engine:max_attempts(#{<<"maxAttempts">> => 0})).
