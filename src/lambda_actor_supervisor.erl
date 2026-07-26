%% OTP root for keyed durable actors. The manager registry and dynamic child
%% supervisor are one coherent generation; if either fails, one_for_all drops
%% stale routing state before accepting another actor request.
-module(lambda_actor_supervisor).
-behaviour(supervisor).

-export([start_link/1, start_actor/2, healthy/0]).
-export([init/1, start_worker_supervisor/0]).

-define(ROOT, ?MODULE).
-define(WORKERS, lambda_actor_worker_supervisor).

start_link(Command) ->
    supervisor:start_link({local, ?ROOT}, ?MODULE, {root, Command}).

start_worker_supervisor() ->
    supervisor:start_link({local, ?WORKERS}, ?MODULE, workers).

start_actor(FunctionRef, ActorKey) ->
    ChildSpec = #{
        id => make_ref(),
        start => {lambda_actor, start_link, [FunctionRef, ActorKey]},
        restart => temporary,
        shutdown => 5000,
        type => worker,
        modules => [lambda_actor]
    },
    supervisor:start_child(?WORKERS, ChildSpec).

healthy() ->
    alive(whereis(?ROOT)) andalso
        alive(whereis(?WORKERS)) andalso
        alive(whereis(lambda_actor_manager)) andalso
        alive(whereis(lambda_actor_alarm)).

init({root, Command}) ->
    Flags = #{
        strategy => one_for_all,
        intensity => 5,
        period => 10
    },
    WorkerSupervisor = #{
        id => actor_workers,
        start => {?MODULE, start_worker_supervisor, []},
        restart => permanent,
        shutdown => infinity,
        type => supervisor,
        modules => [?MODULE]
    },
    Manager = #{
        id => actor_manager,
        start => {lambda_actor_manager, start_link, []},
        restart => permanent,
        shutdown => 10000,
        type => worker,
        modules => [lambda_actor_manager]
    },
    Alarms = #{
        id => actor_alarms,
        start => {lambda_actor_alarm, start_link, [Command]},
        restart => permanent,
        shutdown => 10000,
        type => worker,
        modules => [lambda_actor_alarm]
    },
    {ok, {Flags, [WorkerSupervisor, Manager, Alarms]}};
init(workers) ->
    Flags = #{
        strategy => one_for_one,
        intensity => 100,
        period => 10
    },
    {ok, {Flags, []}}.

alive(Pid) when is_pid(Pid) -> erlang:is_process_alive(Pid);
alive(_) -> false.
