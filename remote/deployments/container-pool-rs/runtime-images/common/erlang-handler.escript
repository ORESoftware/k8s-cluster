#!/usr/bin/env escript
%%! -noshell

main(_) ->
  Runtime = case os:getenv("DD_POOL_RUNTIME") of
    false -> "erlang";
    Value -> Value
  end,
  Input = read_all([]),
  io:format(
    "{\"ok\":true,\"runtime\":\"~s\",\"receivedBytes\":~p}~n",
    [Runtime, byte_size(Input)]
  ).

read_all(Acc) ->
  case io:get_chars("", 4096) of
    eof -> unicode:characters_to_binary(lists:reverse(Acc));
    Data -> read_all([Data | Acc])
  end.
