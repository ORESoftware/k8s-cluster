#!/usr/bin/env escript
%%! -noshell

main(_) ->
  Runtime = case os:getenv("DD_POOL_RUNTIME") of
    false -> "erlang";
    Value -> Value
  end,
  Input = read_all([]),
  Base = io_lib:format(
    "{\"ok\":true,\"runtime\":\"~s\",\"receivedBytes\":~p",
    [json_escape(Runtime), byte_size(Input)]
  ),
  case extract_expr(Input) of
    none ->
      io:format("~s}~n", [Base]);
    Expr ->
      EscapedExpr = json_escape(Expr),
      case eval_expr(Expr) of
        {ok, Answer} ->
          io:format("~s,\"expr\":\"~s\",\"answer\":~p}~n", [Base, EscapedExpr, Answer]);
        {error, Error} ->
          io:format("~s,\"expr\":\"~s\",\"error\":\"~s\"}~n", [Base, EscapedExpr, json_escape(Error)])
      end
  end.

read_all(Acc) ->
  case io:get_chars("", 4096) of
    eof -> unicode:characters_to_binary(lists:reverse(Acc));
    Data -> read_all([Data | Acc])
  end.

extract_expr(Input) ->
  Pattern = "\"(expr|expression)\"\\s*:\\s*\"([^\"]+)\"",
  case re:run(Input, Pattern, [{capture, [2], list}]) of
    {match, [Expr]} -> Expr;
    nomatch -> none
  end.

eval_expr(Expr) ->
  Compact = re:replace(Expr, "\\s+", "", [global, {return, list}]),
  case re:run(Compact, "^(-?[0-9]+)([+*/-])(-?[0-9]+)$", [{capture, [1, 2, 3], list}]) of
    {match, [LeftRaw, Op, RightRaw]} ->
      Left = list_to_integer(LeftRaw),
      Right = list_to_integer(RightRaw),
      case Op of
        "+" -> {ok, Left + Right};
        "-" -> {ok, Left - Right};
        "*" -> {ok, Left * Right};
        "/" when Right =:= 0 -> {error, "division by zero"};
        "/" -> {ok, Left div Right}
      end;
    nomatch ->
      {error, "unsupported expression"}
  end.

json_escape(Value) when is_binary(Value) ->
  json_escape(binary_to_list(Value));
json_escape([]) ->
  [];
json_escape([$" | Rest]) ->
  [$\\, $" | json_escape(Rest)];
json_escape([$\\ | Rest]) ->
  [$\\, $\\ | json_escape(Rest)];
json_escape([$\n | Rest]) ->
  [$\\, $n | json_escape(Rest)];
json_escape([$\r | Rest]) ->
  [$\\, $r | json_escape(Rest)];
json_escape([$\t | Rest]) ->
  [$\\, $t | json_escape(Rest)];
json_escape([Char | Rest]) when Char < 32 ->
  [$\s | json_escape(Rest)];
json_escape([Char | Rest]) ->
  [Char | json_escape(Rest)].
