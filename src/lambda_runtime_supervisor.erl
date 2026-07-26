-module(lambda_runtime_supervisor).
-behaviour(supervisor).

-export([
    start_link/0,
    ensure_started/0,
    start_worker/1,
    healthy/0,
    draining/0,
    snapshot/0,
    metrics/0
]).
-export([init/1, start_worker_supervisor/0]).

-define(ROOT, lambda_runtime_supervisor).
-define(WORKER_SUPERVISOR, lambda_runtime_worker_supervisor).
-define(MANAGER, lambda_child_runner_manager).

%% The root is part of the Gleam static supervision tree. one_for_all is
%% deliberate: the manager owns the ETS routing table, so losing either the
%% manager or worker supervisor must replace both and rebuild one coherent
%% generation instead of leaving orphaned warm processes behind.
start_link() ->
    supervisor:start_link({local, ?ROOT}, ?MODULE, root).

%% Direct FFI tests can reach the child runner without starting the HTTP
%% application. Keep that path supervised too, while production starts this
%% module as a child of the Gleam root supervisor.
ensure_started() ->
    case whereis(?ROOT) of
        Pid when is_pid(Pid) ->
            ok;
        undefined ->
            case start_link() of
                {ok, Pid} ->
                    unlink(Pid),
                    ok;
                {error, {already_started, _Pid}} ->
                    ok;
                {error, Reason} ->
                    {error, Reason}
            end
    end.

start_worker_supervisor() ->
    supervisor:start_link({local, ?WORKER_SUPERVISOR}, ?MODULE, workers).

%% Every warm runtime is a temporary dynamic child. A failed runtime is removed
%% from the manager registry by its monitor and recreated by the next invoke;
%% it never takes down unrelated runtimes or the HTTP server.
start_worker(Command) ->
    ChildSpec = #{
        id => make_ref(),
        start => {lambda_child_runner, start_worker_link, [Command]},
        restart => temporary,
        shutdown => 5000,
        type => worker,
        modules => [lambda_child_runner]
    },
    supervisor:start_child(?WORKER_SUPERVISOR, ChildSpec).

init(root) ->
    Flags = #{
        strategy => one_for_all,
        intensity => 5,
        period => 10
    },
    WorkerSupervisor = #{
        id => runtime_workers,
        start => {?MODULE, start_worker_supervisor, []},
        restart => permanent,
        shutdown => infinity,
        type => supervisor,
        modules => [?MODULE]
    },
    Manager = #{
        id => runtime_manager,
        start => {lambda_child_runner, start_manager_link, []},
        restart => permanent,
        shutdown => 10000,
        type => worker,
        modules => [lambda_child_runner]
    },
    {ok, {Flags, [WorkerSupervisor, Manager]}};
init(workers) ->
    Flags = #{
        strategy => one_for_one,
        intensity => 100,
        period => 10
    },
    {ok, {Flags, []}}.

healthy() ->
    process_alive(whereis(?ROOT)) andalso
        process_alive(whereis(?WORKER_SUPERVISOR)) andalso
        process_alive(whereis(?MANAGER)).

draining() ->
    Marker = case os:getenv("SCINTILLA_DRAIN_MARKER") of
        false -> "/tmp/scintilla-draining";
        Value -> Value
    end,
    filelib:is_regular(Marker).

snapshot() ->
    Counts = worker_counts(),
    Active = maps:get(active, Counts, 0),
    Specs = maps:get(specs, Counts, 0),
    iolist_to_binary([
        "{\"ok\":", bool_json(healthy()),
        ",\"supervisionStrategy\":\"one_for_all\"",
        ",\"dynamicChildren\":true",
        ",\"draining\":", bool_json(draining()),
        ",\"activeWorkers\":", integer_to_binary(Active),
        ",\"workerSpecs\":", integer_to_binary(Specs),
        "}"
    ]).

metrics() ->
    Counts = worker_counts(),
    iolist_to_binary([
        "# HELP dd_lambda_runner_supervisor_up Whether the BEAM runtime supervision tree is healthy.\n",
        "# TYPE dd_lambda_runner_supervisor_up gauge\n",
        metric_line("dd_lambda_runner_supervisor_up", bool_int(healthy())),
        "# HELP dd_lambda_runner_supervised_workers Active dynamic runtime children supervised by OTP.\n",
        "# TYPE dd_lambda_runner_supervised_workers gauge\n",
        metric_line("dd_lambda_runner_supervised_workers", maps:get(active, Counts, 0)),
        "# HELP dd_lambda_runner_draining Whether this runner has stopped accepting new traffic.\n",
        "# TYPE dd_lambda_runner_draining gauge\n",
        metric_line("dd_lambda_runner_draining", bool_int(draining()))
    ]).

worker_counts() ->
    case whereis(?WORKER_SUPERVISOR) of
        Pid when is_pid(Pid) ->
            try maps:from_list(supervisor:count_children(Pid)) of
                Counts -> Counts
            catch
                _:_ -> #{}
            end;
        undefined ->
            #{}
    end.

process_alive(Pid) when is_pid(Pid) ->
    erlang:is_process_alive(Pid);
process_alive(_) ->
    false.

bool_json(true) -> "true";
bool_json(false) -> "false".

bool_int(true) -> 1;
bool_int(false) -> 0.

metric_line(Name, Value) ->
    io_lib:format("~s{service=\"dd-gleam-lambda-runner\"} ~p~n", [Name, Value]).
