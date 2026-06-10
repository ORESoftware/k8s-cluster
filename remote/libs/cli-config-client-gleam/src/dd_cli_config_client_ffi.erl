%%% Shared CLI/env config reconciler for Gleam deployments.
%%%
%%% The public contract mirrors ORESoftware/flags-2-env's .cli-flags.toml
%%% shape, but keeps state in one local BEAM owner process. The owner writes
%%% persistent_term once at boot; readers do cheap persistent_term lookups.

-module(dd_cli_config_client_ffi).

-export([
    load_once/0,
    reload/0,
    env/1,
    getenv/2,
    string/2,
    int/2,
    bool/2,
    source/1,
    snapshot_json/0
]).

-define(OWNER, dd_cli_config_client_owner).
-define(VALUES_KEY, {?MODULE, values}).
-define(SOURCES_KEY, {?MODULE, sources}).
-define(CONFIG_PATH_KEY, {?MODULE, config_path}).
-define(FLAGS_KEY, {?MODULE, flags}).

load_once() ->
    case persistent_term:get(?VALUES_KEY, undefined) of
        undefined -> ensure_owner_loaded();
        _ -> nil
    end.

reload() ->
    ensure_owner_reloaded().

env(Name) ->
    load_once(),
    Key = to_binary(Name),
    case maps:get(Key, persistent_term:get(?VALUES_KEY, #{}), undefined) of
        undefined -> {error, nil};
        Value -> {ok, Value}
    end.

getenv(Name, Fallback) ->
    case env(Name) of
        {ok, <<>>} -> Fallback;
        {ok, Value} -> Value;
        {error, nil} -> Fallback
    end.

string(Name, Fallback) ->
    getenv(Name, Fallback).

int(Name, Fallback) ->
    case env(Name) of
        {ok, Value} ->
            try binary_to_integer(Value)
            catch _:_ -> Fallback
            end;
        {error, nil} -> Fallback
    end.

bool(Name, Fallback) ->
    case env(Name) of
        {ok, Value} -> parse_bool(Value, Fallback);
        {error, nil} -> Fallback
    end.

source(Name) ->
    load_once(),
    Key = to_binary(Name),
    maps:get(Key, persistent_term:get(?SOURCES_KEY, #{}), <<"missing">>).

snapshot_json() ->
    load_once(),
    Values = persistent_term:get(?VALUES_KEY, #{}),
    Sources = persistent_term:get(?SOURCES_KEY, #{}),
    Path = persistent_term:get(?CONFIG_PATH_KEY, undefined),
    Entries = maps:fold(
        fun(Key, Value, Acc) ->
            Source = maps:get(Key, Sources, <<"missing">>),
            [
                [
                    "{\"key\":\"", escape_json(Key),
                    "\",\"value\":\"", escape_json(Value),
                    "\",\"source\":\"", escape_json(Source), "\"}"
                ]
                | Acc
            ]
        end,
        [],
        Values
    ),
    EntryJson = join_iolist(lists:reverse(Entries), ","),
    PathJson = case Path of
        undefined -> "null";
        _ -> ["\"", escape_json(to_binary(Path)), "\""]
    end,
    iolist_to_binary([
        "{\"ok\":true,\"configPath\":", PathJson,
        ",\"entries\":[", EntryJson, "]}"
    ]).

%% ---------- owner -------------------------------------------------------

ensure_owner_loaded() ->
    call_owner(load),
    nil.

ensure_owner_reloaded() ->
    call_owner(reload),
    nil.

call_owner(Action) ->
    Parent = self(),
    case whereis(?OWNER) of
        undefined ->
            Pid = spawn(fun() -> owner_boot(Action, Parent) end),
            wait_loaded(Pid);
        Pid ->
            Pid ! {Action, Parent},
            wait_loaded(Pid)
    end.

owner_boot(Action, Parent) ->
    case catch register(?OWNER, self()) of
        true ->
            apply_snapshot(),
            Parent ! {?MODULE, loaded, self()},
            owner_loop();
        _ ->
            case whereis(?OWNER) of
                undefined ->
                    Parent ! {?MODULE, loaded, self()};
                Pid ->
                    Pid ! {Action, Parent}
            end
    end.

owner_loop() ->
    receive
        {reload, From} ->
            apply_snapshot(),
            From ! {?MODULE, loaded, self()},
            owner_loop();
        {load, From} ->
            From ! {?MODULE, loaded, self()},
            owner_loop()
    end.

wait_loaded(Pid) ->
    receive
        {?MODULE, loaded, _} -> ok
    after 5000 ->
        io:format("[cli-config] timed out waiting for owner ~p~n", [Pid]),
        ok
    end.

apply_snapshot() ->
    Env = env_map(),
    ConfigPath = config_path(Env),
    Flags = read_flags(ConfigPath),
    {DefaultValues, DefaultSources} = defaults_from_flags(Flags),
    EnvSources = map_values(fun(_) -> <<"env">> end, Env),
    CliValues = parse_args(init:get_plain_arguments(), Flags),
    CliSources = map_values(fun(_) -> <<"cli">> end, CliValues),
    Values = maps:merge(maps:merge(DefaultValues, Env), CliValues),
    Sources = maps:merge(maps:merge(DefaultSources, EnvSources), CliSources),
    persistent_term:put(?VALUES_KEY, Values),
    persistent_term:put(?SOURCES_KEY, Sources),
    persistent_term:put(?CONFIG_PATH_KEY, ConfigPath),
    persistent_term:put(?FLAGS_KEY, Flags),
    io:format(
        "[cli-config] loaded config_path=~ts flags=~p cli_overrides=~p~n",
        [printable_path(ConfigPath), length(Flags), maps:size(CliValues)]
    ).

%% ---------- environment + config file ----------------------------------

env_map() ->
    lists:foldl(fun env_entry_to_map/2, #{}, os:getenv()).

env_entry_to_map(Entry, Acc) ->
    case string:split(Entry, "=", leading) of
        [Key, Value] ->
            maps:put(to_binary(Key), to_binary(Value), Acc);
        _ ->
            Acc
    end.

config_path(Env) ->
    case maps:get(<<"FLAGS2ENV_CONFIG">>, Env, undefined) of
        undefined -> discover_config();
        <<>> -> discover_config();
        Path -> binary_to_list(Path)
    end.

discover_config() ->
    {ok, Cwd} = file:get_cwd(),
    Home = case os:getenv("HOME") of
        false -> "";
        H -> filename:absname(H)
    end,
    discover_config_from(filename:absname(Cwd), Home).

discover_config_from(Dir, Home) ->
    Candidate = filename:join(Dir, ".cli-flags.toml"),
    case filelib:is_file(Candidate) andalso filename:absname(Dir) =/= Home of
        true -> Candidate;
        false ->
            Parent = filename:dirname(Dir),
            case Parent =:= Dir of
                true -> undefined;
                false -> discover_config_from(Parent, Home)
            end
    end.

read_flags(undefined) ->
    [];
read_flags(Path) ->
    case file:read_file(Path) of
        {ok, Body} ->
            Lines0 = binary:split(Body, <<"\n">>, [global]),
            Lines = [binary_to_list(Line) || Line <- Lines0],
            {Flags, Current} =
                lists:foldl(fun parse_config_line/2, {[], none}, Lines),
            lists:reverse(maybe_add_flag(Current, Flags));
        {error, Reason} ->
            io:format("[cli-config] could not read ~ts: ~p~n", [Path, Reason]),
            []
    end.

parse_config_line(Line0, {Flags, Current}) ->
    Line = string:trim(strip_comment(Line0)),
    case Line of
        "" ->
            {Flags, Current};
        [$[, $f, $l, $a, $g, $s, $. | Rest] ->
            Name = string:trim(string:trim(Rest, trailing, "]")),
            NewFlags = maybe_add_flag(Current, Flags),
            {NewFlags, new_flag(Name)};
        [$[ | _] ->
            {maybe_add_flag(Current, Flags), none};
        _ ->
            case Current of
                none -> {Flags, Current};
                Flag -> {Flags, apply_flag_field(Line, Flag)}
            end
    end.

strip_comment(Line) ->
    case string:split(Line, "#", leading) of
        [Before, _] -> Before;
        [Only] -> Only
    end.

new_flag(Name) ->
    #{
        id => to_binary(Name),
        env => undefined,
        aliases => [],
        short => undefined,
        type => <<"string">>,
        default => undefined,
        true_aliases => [],
        false_aliases => []
    }.

apply_flag_field(Line, Flag) ->
    case string:split(Line, "=", leading) of
        [Key0, Value0] ->
            Key = string:trim(Key0),
            Value = string:trim(Value0),
            case Key of
                "env" -> Flag#{env => scalar(Value)};
                "aliases" -> Flag#{aliases => list_value(Value)};
                "short" -> Flag#{short => scalar(Value)};
                "type" -> Flag#{type => lower_bin(scalar(Value))};
                "default" -> Flag#{default => scalar(Value)};
                "true_aliases" -> Flag#{true_aliases => list_value(Value)};
                "false_aliases" -> Flag#{false_aliases => list_value(Value)};
                _ -> Flag
            end;
        _ -> Flag
    end.

maybe_add_flag(none, Flags) ->
    Flags;
maybe_add_flag(#{env := undefined}, Flags) ->
    Flags;
maybe_add_flag(Flag, Flags) ->
    [Flag | Flags].

scalar(Value0) ->
    Value = string:trim(Value0),
    case Value of
        [$" | Rest] ->
            to_binary(string:trim(Rest, trailing, "\""));
        "true" -> <<"true">>;
        "false" -> <<"false">>;
        _ -> to_binary(Value)
    end.

list_value(Value0) ->
    Value1 = string:trim(Value0),
    Value = string:trim(string:trim(Value1, leading, "["), trailing, "]"),
    case Value of
        "" -> [];
        _ ->
            Parts = string:split(Value, ",", all),
            [scalar(Part) || Part <- Parts]
    end.

defaults_from_flags(Flags) ->
    lists:foldl(
        fun(Flag, {Values, Sources}) ->
            case maps:get(default, Flag, undefined) of
                undefined -> {Values, Sources};
                Default ->
                    Env = maps:get(env, Flag),
                    case validate_value(Flag, Default) of
                        {ok, Value} ->
                            {
                                maps:put(Env, Value, Values),
                                maps:put(Env, <<"default">>, Sources)
                            };
                        error -> {Values, Sources}
                    end
            end
        end,
        {#{}, #{}},
        Flags
    ).

%% ---------- argv parsing ------------------------------------------------

parse_args(Argv0, Flags) ->
    Argv = [to_binary(A) || A <- Argv0],
    AliasMap = alias_map(Flags),
    ShortMap = short_map(Flags),
    parse_tokens(Argv, AliasMap, ShortMap, #{}).

alias_map(Flags) ->
    lists:foldl(
        fun(Flag, Acc0) ->
            Names = [maps:get(id, Flag) | maps:get(aliases, Flag, [])],
            lists:foldl(
                fun(Name, Acc) -> maps:put(Name, Flag, Acc) end,
                Acc0,
                Names
            )
        end,
        #{},
        Flags
    ).

short_map(Flags) ->
    lists:foldl(
        fun(Flag, Acc) ->
            case maps:get(short, Flag, undefined) of
                undefined -> Acc;
                <<>> -> Acc;
                Short -> maps:put(Short, Flag, Acc)
            end
        end,
        #{},
        Flags
    ).

parse_tokens([], _AliasMap, _ShortMap, Acc) ->
    Acc;
parse_tokens([<<"--">> | _], _AliasMap, _ShortMap, Acc) ->
    Acc;
parse_tokens([<<"--no-", Name/binary>> | Rest], AliasMap, ShortMap, Acc) ->
    case maps:get(Name, AliasMap, undefined) of
        #{type := <<"bool">>} = Flag ->
            parse_tokens(Rest, AliasMap, ShortMap, put_cli(Flag, <<"false">>, Acc));
        _ ->
            parse_tokens(Rest, AliasMap, ShortMap, Acc)
    end;
parse_tokens([<<"--", Long/binary>> | Rest], AliasMap, ShortMap, Acc) ->
    {Name, Inline} = split_inline(Long),
    case maps:get(Name, AliasMap, undefined) of
        undefined ->
            parse_tokens(Rest, AliasMap, ShortMap, Acc);
        #{type := <<"bool">>} = Flag ->
            parse_bool_flag(Flag, Inline, Rest, AliasMap, ShortMap, Acc);
        Flag ->
            parse_value_flag(Flag, Inline, Rest, AliasMap, ShortMap, Acc)
    end;
parse_tokens([<<"-", ShortAndValue/binary>> | Rest], AliasMap, ShortMap, Acc)
    when byte_size(ShortAndValue) > 0 ->
    parse_short(ShortAndValue, Rest, AliasMap, ShortMap, Acc);
parse_tokens([_ | Rest], AliasMap, ShortMap, Acc) ->
    parse_tokens(Rest, AliasMap, ShortMap, Acc).

parse_bool_flag(Flag, Inline, Rest, AliasMap, ShortMap, Acc) ->
    case Inline of
        undefined ->
            case Rest of
                [Next | Tail] ->
                    case validate_value(Flag, Next) of
                        {ok, Value} ->
                            parse_tokens(Tail, AliasMap, ShortMap, put_cli(Flag, Value, Acc));
                        error ->
                            parse_tokens(Rest, AliasMap, ShortMap, put_cli(Flag, <<"true">>, Acc))
                    end;
                [] ->
                    parse_tokens(Rest, AliasMap, ShortMap, put_cli(Flag, <<"true">>, Acc))
            end;
        Value0 ->
            case validate_value(Flag, Value0) of
                {ok, Value} ->
                    parse_tokens(Rest, AliasMap, ShortMap, put_cli(Flag, Value, Acc));
                error ->
                    parse_tokens(Rest, AliasMap, ShortMap, Acc)
            end
    end.

parse_value_flag(Flag, Inline, Rest, AliasMap, ShortMap, Acc) ->
    case Inline of
        undefined ->
            case Rest of
                [Next | Tail] ->
                    case value_token_ok(Flag, Next) of
                        true ->
                            case validate_value(Flag, Next) of
                                {ok, Value} ->
                                    parse_tokens(
                                        Tail,
                                        AliasMap,
                                        ShortMap,
                                        put_cli(Flag, Value, Acc)
                                    );
                                error ->
                                    parse_tokens(Tail, AliasMap, ShortMap, Acc)
                            end;
                        false ->
                            parse_tokens(Rest, AliasMap, ShortMap, Acc)
                    end;
                [] ->
                    parse_tokens(Rest, AliasMap, ShortMap, Acc)
            end;
        Value0 ->
            case validate_value(Flag, Value0) of
                {ok, Value} ->
                    parse_tokens(Rest, AliasMap, ShortMap, put_cli(Flag, Value, Acc));
                error ->
                    parse_tokens(Rest, AliasMap, ShortMap, Acc)
            end
    end.

parse_short(ShortAndValue, Rest, AliasMap, ShortMap, Acc) ->
    <<ShortChar, Tail/binary>> = ShortAndValue,
    Short = <<ShortChar>>,
    case maps:get(Short, ShortMap, undefined) of
        undefined ->
            parse_tokens(Rest, AliasMap, ShortMap, Acc);
        #{type := <<"bool">>} = Flag ->
            case Tail of
                <<>> ->
                    parse_tokens(Rest, AliasMap, ShortMap, put_cli(Flag, <<"true">>, Acc));
                <<"=", Inline/binary>> ->
                    case validate_value(Flag, Inline) of
                        {ok, Value} ->
                            parse_tokens(Rest, AliasMap, ShortMap, put_cli(Flag, Value, Acc));
                        error ->
                            parse_tokens(Rest, AliasMap, ShortMap, Acc)
                    end;
                _ ->
                    parse_short_cluster(Tail, Rest, AliasMap, ShortMap, put_cli(Flag, <<"true">>, Acc))
            end;
        Flag ->
            Inline = case Tail of
                <<"=", V/binary>> -> V;
                <<>> -> undefined;
                _ -> Tail
            end,
            parse_value_flag(Flag, Inline, Rest, AliasMap, ShortMap, Acc)
    end.

parse_short_cluster(<<>>, Rest, AliasMap, ShortMap, Acc) ->
    parse_tokens(Rest, AliasMap, ShortMap, Acc);
parse_short_cluster(<<ShortChar, Tail/binary>>, Rest, AliasMap, ShortMap, Acc) ->
    Short = <<ShortChar>>,
    case maps:get(Short, ShortMap, undefined) of
        #{type := <<"bool">>} = Flag ->
            parse_short_cluster(Tail, Rest, AliasMap, ShortMap, put_cli(Flag, <<"true">>, Acc));
        _ ->
            parse_tokens(Rest, AliasMap, ShortMap, Acc)
    end.

split_inline(Bin) ->
    case binary:split(Bin, <<"=">>) of
        [Name, Value] -> {Name, Value};
        [Name] -> {Name, undefined}
    end.

value_token_ok(#{type := <<"integer">>}, Value) ->
    not option_token(Value) orelse integer_text(Value);
value_token_ok(#{type := <<"int">>}, Value) ->
    not option_token(Value) orelse integer_text(Value);
value_token_ok(_Flag, Value) ->
    not option_token(Value).

option_token(<<"-", _/binary>>) -> true;
option_token(_) -> false.

put_cli(Flag, Value, Acc) ->
    maps:put(maps:get(env, Flag), Value, Acc).

validate_value(#{type := Type} = Flag, Value0) ->
    Value = trim_bin(Value0),
    case Type of
        <<"bool">> -> validate_bool(Flag, Value);
        <<"integer">> -> validate_integer(Value);
        <<"int">> -> validate_integer(Value);
        <<"json">> -> validate_jsonish(Value);
        _ -> {ok, Value}
    end.

validate_bool(Flag, Value) ->
    Lower = lower_bin(Value),
    TrueAliases = [<<"true">> | [lower_bin(A) || A <- maps:get(true_aliases, Flag, [])]],
    FalseAliases = [<<"false">> | [lower_bin(A) || A <- maps:get(false_aliases, Flag, [])]],
    case lists:member(Lower, TrueAliases) of
        true -> {ok, <<"true">>};
        false ->
            case lists:member(Lower, FalseAliases) of
                true -> {ok, <<"false">>};
                false -> error
            end
    end.

validate_integer(Value) ->
    case integer_text(Value) of
        true -> {ok, Value};
        false -> error
    end.

integer_text(Value) ->
    try
        _ = binary_to_integer(Value),
        true
    catch _:_ ->
        false
    end.

validate_jsonish(<<First, _/binary>> = Value)
    when First =:= ${; First =:= $[; First =:= $"; First =:= $t;
         First =:= $f; First =:= $n; First =:= $-;
         (First >= $0 andalso First =< $9) ->
    {ok, Value};
validate_jsonish(_) ->
    error.

parse_bool(Value, Fallback) ->
    case lower_bin(to_binary(Value)) of
        <<"1">> -> true;
        <<"true">> -> true;
        <<"yes">> -> true;
        <<"on">> -> true;
        <<"0">> -> false;
        <<"false">> -> false;
        <<"no">> -> false;
        <<"off">> -> false;
        _ -> Fallback
    end.

%% ---------- utilities ---------------------------------------------------

map_values(Fun, Map) ->
    maps:fold(fun(K, V, Acc) -> maps:put(K, Fun(V), Acc) end, #{}, Map).

to_binary(B) when is_binary(B) -> B;
to_binary(A) when is_atom(A) -> atom_to_binary(A, utf8);
to_binary(I) when is_integer(I) -> integer_to_binary(I);
to_binary(L) when is_list(L) -> unicode:characters_to_binary(L).

lower_bin(Bin) ->
    to_binary(string:lowercase(binary_to_list(to_binary(Bin)))).

trim_bin(Bin) ->
    to_binary(string:trim(binary_to_list(to_binary(Bin)))).

printable_path(undefined) -> <<"none">>;
printable_path(Path) -> to_binary(Path).

join_iolist([], _Sep) ->
    [];
join_iolist([One], _Sep) ->
    One;
join_iolist([One | Rest], Sep) ->
    [One, Sep, join_iolist(Rest, Sep)].

escape_json(Value) ->
    escape_json(to_binary(Value), []).

escape_json(<<>>, Acc) ->
    lists:reverse(Acc);
escape_json(<<"\\", Rest/binary>>, Acc) ->
    escape_json(Rest, [$\\, $\\ | Acc]);
escape_json(<<"\"", Rest/binary>>, Acc) ->
    escape_json(Rest, [$", $\\ | Acc]);
escape_json(<<"\n", Rest/binary>>, Acc) ->
    escape_json(Rest, [$n, $\\ | Acc]);
escape_json(<<"\r", Rest/binary>>, Acc) ->
    escape_json(Rest, [$r, $\\ | Acc]);
escape_json(<<"\t", Rest/binary>>, Acc) ->
    escape_json(Rest, [$t, $\\ | Acc]);
escape_json(<<C, Rest/binary>>, Acc) ->
    escape_json(Rest, [C | Acc]).
