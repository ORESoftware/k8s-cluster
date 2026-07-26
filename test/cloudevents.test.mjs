import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import test from 'node:test';

const root = resolve(import.meta.dirname, '..');

function commandExists(command) {
  return spawnSync(command, ['-version'], { stdio: 'ignore' }).error === undefined;
}

test('CloudEvents filters fan out and idempotency is stable', {
  skip: !commandExists('erlc'),
}, async () => {
  const build = await mkdtemp(join(tmpdir(), 'scintilla-events-pure-'));
  execFileSync('erlc', [
    '-o',
    build,
    join(root, 'src/lambda_events.erl'),
  ]);

  const probe = `
EventJson = <<"{\\"specversion\\":\\"1.0\\",\\"id\\":\\"evt-42\\",\\"source\\":\\"/shops/chi/orders\\",\\"type\\":\\"dev.scintilla.order.created.v1\\",\\"subject\\":\\"ord-7\\",\\"tenant\\":\\"acme\\",\\"data\\":{\\"total\\":42}}">>,
{ok, Event} = lambda_events:validate_event(EventJson),
{error, _} = lambda_events:validate_event(<<"{}">>),
{error, _} = lambda_events:validate_event(<<"{\\"specversion\\":\\"0.3\\"}">>),
Functions = [
  #{
    <<"id">> => <<"11111111-1111-1111-1111-111111111111">>,
    <<"metaData">> => #{
      <<"eventBindings">> => [
        #{
          <<"name">> => <<"orders">>,
          <<"type">> => <<"dev.scintilla.order.created.v1">>,
          <<"sourcePrefix">> => <<"/shops/">>,
          <<"attributes">> => #{<<"tenant">> => <<"acme">>}
        },
        #{<<"name">> => <<"all-scintilla">>, <<"typePrefix">> => <<"dev.scintilla.">>},
        #{<<"name">> => <<"disabled">>, <<"enabled">> => false},
        #{<<"name">> => <<"wrong-tenant">>, <<"attributes">> => #{<<"tenant">> => <<"other">>}}
      ]
    }
  },
  #{
    <<"id">> => <<"22222222-2222-2222-2222-222222222222">>,
    <<"metaData">> => #{
      <<"eventBindings">> => [#{<<"subject">> => <<"other-order">>}]
    }
  }
],
Targets = lambda_events:matching_targets(Functions, Event),
2 = length(Targets),
Key1 = lambda_events:idempotency_key(<<"fn">>, <<"orders">>, <<"evt-42">>, <<"/shops/chi/orders">>, <<"order.created">>),
Key1 = lambda_events:idempotency_key(<<"fn">>, <<"orders">>, <<"evt-42">>, <<"/shops/chi/orders">>, <<"order.created">>),
Key2 = lambda_events:idempotency_key(<<"fn">>, <<"other">>, <<"evt-42">>, <<"/shops/chi/orders">>, <<"order.created">>),
false = Key1 =:= Key2,
70 = byte_size(Key1),
io:format("CLOUDEVENTS_PURE_OK~n"),
halt(0).
`;
  const result = spawnSync('erl', ['-noshell', '-pa', build, '-eval', probe], {
    encoding: 'utf8',
    timeout: 10_000,
  });
  assert.equal(result.status, 0, result.stderr || result.stdout);
  assert.match(result.stdout, /CLOUDEVENTS_PURE_OK/);
});

test('supervised event router bounds concurrent routing jobs', {
  skip: !commandExists('erlc'),
}, async () => {
  const build = await mkdtemp(join(tmpdir(), 'scintilla-events-router-'));
  const storeStub = join(build, 'workflow_store.erl');
  const asyncStub = join(build, 'lambda_async.erl');
  const engineStub = join(build, 'workflow_engine.erl');

  await Promise.all([
    writeFile(storeStub, `
-module(workflow_store).
-export([list_event_bound_functions/0]).
list_event_bound_functions() ->
  {ok, [#{
    <<"id">> => <<"11111111-1111-1111-1111-111111111111">>,
    <<"metaData">> => #{<<"eventBindings">> => [
      #{<<"name">> => <<"slow">>, <<"type">> => <<"test.slow">>},
      #{<<"name">> => <<"overflow-a">>, <<"type">> => <<"test.overflow">>},
      #{<<"name">> => <<"overflow-b">>, <<"type">> => <<"test.overflow">>}
    ]}
  }]}.
`),
    writeFile(asyncStub, `
-module(lambda_async).
-export([start_from_body/2]).
start_from_body(_FunctionId, _Body) ->
  timer:sleep(250),
  {ok, <<"{}">>}.
`),
    writeFile(engineStub, `
-module(workflow_engine).
-export([enabled/0]).
enabled() -> true.
`),
  ]);

  execFileSync('erlc', [
    '-o',
    build,
    join(root, 'src/lambda_events.erl'),
    storeStub,
    asyncStub,
    engineStub,
  ]);

  const probe = `
true = os:putenv("EVENT_ROUTER_MAX_INFLIGHT", "1"),
true = os:putenv("EVENT_ROUTER_MAX_TARGETS", "1"),
{ok, Router} = lambda_events:start_link(),
unlink(Router),
Event = <<"{\\"specversion\\":\\"1.0\\",\\"id\\":\\"evt-1\\",\\"source\\":\\"/test\\",\\"type\\":\\"test.slow\\"}">>,
Parent = self(),
spawn(fun() -> Parent ! {first, lambda_events:route_from_body(Event)} end),
timer:sleep(50),
{error, <<"CloudEvents router concurrency limit reached">>} = lambda_events:route_from_body(Event),
receive
  {first, {ok, Summary}} ->
    true = binary:match(Summary, <<"\\"accepted\\":1">>) =/= nomatch,
    true = binary:match(Summary, <<"\\"matched\\":1">>) =/= nomatch,
    true = binary:match(Summary, <<"\\"overflow\\":0">>) =/= nomatch
after 3000 ->
  erlang:error(first_route_timeout)
end,
OverflowEvent = <<"{\\"specversion\\":\\"1.0\\",\\"id\\":\\"evt-2\\",\\"source\\":\\"/test\\",\\"type\\":\\"test.overflow\\"}">>,
{error, <<"CloudEvent target limit exceeded: matched 2, maximum 1">>} =
  lambda_events:route_from_body(OverflowEvent),
exit(Router, shutdown),
io:format("CLOUDEVENTS_ROUTER_OK~n"),
halt(0).
`;
  const result = spawnSync('erl', ['-noshell', '-pa', build, '-eval', probe], {
    encoding: 'utf8',
    timeout: 10_000,
  });
  assert.equal(result.status, 0, result.stderr || result.stdout);
  assert.match(result.stdout, /CLOUDEVENTS_ROUTER_OK/);
});

test('HTTP and NATS use the same supervised durable CloudEvents router', async () => {
  const [store, events, app, http, nats, deployment] = await Promise.all([
    readFile(join(root, 'src/workflow_store.erl'), 'utf8'),
    readFile(join(root, 'src/lambda_events.erl'), 'utf8'),
    readFile(join(root, 'src/gleam_lambda_runner.gleam'), 'utf8'),
    readFile(join(root, 'src/gleam_lambda_runner/http_server.gleam'), 'utf8'),
    readFile(join(root, 'src/lambda_nats.erl'), 'utf8'),
    readFile(
      join(root, 'k8s/ec2/dd-gleam-lambda-runner.deployment.yaml'),
      'utf8',
    ),
  ]);

  assert.match(
    store,
    /'gleam_lambda_runner@pg_contract':lambda_functions_select_sql\(\)/,
  );
  assert.match(store, /meta_data_json::jsonb \? 'eventBindings'/);
  assert.match(events, /lambda_async:start_from_body/);
  assert.match(events, /spawn_monitor/);
  assert.match(events, /EVENT_ROUTER_MAX_INFLIGHT/);
  assert.match(app, /supervisor\.add\(events\.supervised\(\)\)/);
  assert.match(http, /Post, \["events"\]/);
  assert.match(nats, /NATS_CLOUDEVENTS_SUBJECT/);
  assert.match(nats, /lambda_events:route_from_body\(Payload\)/);
  assert.match(deployment, /EVENT_ROUTER_MAX_TARGETS/);
});
