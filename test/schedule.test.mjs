import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtemp, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import test from 'node:test';

const root = resolve(import.meta.dirname, '..');

function commandExists(command) {
  return spawnSync(command, ['-version'], { stdio: 'ignore' }).error === undefined;
}

test('UTC cron matching supports aliases, lists, ranges, steps, and cron day semantics', {
  skip: !commandExists('erlc'),
}, async () => {
  const build = await mkdtemp(join(tmpdir(), 'scintilla-schedule-'));
  execFileSync('erlc', [
    '-o',
    build,
    join(root, 'src/lambda_schedule.erl'),
  ]);

  const probe = `
DateTime = {{2026, 7, 26}, {4, 30, 17}},
true = lambda_schedule:cron_matches(<<"* * * * *">>, DateTime),
true = lambda_schedule:cron_matches(<<"*/15 4 * * *">>, DateTime),
true = lambda_schedule:cron_matches(<<"0,30 4 1-31/5 * *">>, DateTime),
false = lambda_schedule:cron_matches(<<"29 4 * * *">>, DateTime),
false = lambda_schedule:cron_matches(<<"@hourly">>, DateTime),
true = lambda_schedule:cron_matches(<<"30 4 * * 0">>, DateTime),
true = lambda_schedule:cron_matches(<<"30 4 * * 7">>, DateTime),
true = lambda_schedule:cron_matches(<<"30 4 1 * 0">>, DateTime),
false = lambda_schedule:cron_matches(<<"30 4 1 * 1">>, DateTime),
false = lambda_schedule:cron_matches(<<"bad cron">>, DateTime),
false = lambda_schedule:cron_matches(<<"1//2 4 * * *">>, DateTime),
false = lambda_schedule:cron_matches(<<"30 4 * * -1">>, DateTime),
false = lambda_schedule:cron_matches(<<"0,,30 4 * * *">>, DateTime),
Function = #{
  <<"id">> => <<"11111111-1111-1111-1111-111111111111">>,
  <<"slug">> => <<"scheduled-test">>,
  <<"metaData">> => #{
    <<"schedules">> => [
      #{<<"name">> => <<"due">>, <<"cron">> => <<"30 4 * * *">>, <<"payload">> => #{<<"n">> => 1}},
      #{<<"name">> => <<"disabled">>, <<"cron">> => <<"* * * * *">>, <<"enabled">> => false},
      #{<<"name">> => <<"wrong-zone">>, <<"cron">> => <<"* * * * *">>, <<"timezone">> => <<"America/Chicago">>}
    ]
  }
},
[{Function, Due, 0}] = lambda_schedule:due_events([Function], DateTime),
<<"due">> = maps:get(<<"name">>, Due),
io:format("SCHEDULE_OK~n"),
halt(0).
`;
  const result = spawnSync('erl', ['-noshell', '-pa', build, '-eval', probe], {
    encoding: 'utf8',
    timeout: 10_000,
  });
  assert.equal(result.status, 0, result.stderr || result.stdout);
  assert.match(result.stdout, /SCHEDULE_OK/);
});

test('schedule discovery uses canonical contracts and the durable async path', async () => {
  const store = await readFile(join(root, 'src/workflow_store.erl'), 'utf8');
  const scheduler = await readFile(join(root, 'src/lambda_schedule.erl'), 'utf8');
  const app = await readFile(join(root, 'src/gleam_lambda_runner.gleam'), 'utf8');

  assert.match(
    store,
    /'gleam_lambda_runner@pg_contract':lambda_functions_select_sql\(\)/,
  );
  assert.match(scheduler, /lambda_async:start_from_body/);
  assert.match(scheduler, /"cron:"/);
  assert.match(scheduler, /"specversion">> => <<"1\.0"/);
  assert.match(app, /supervisor\.add\(schedule\.supervised\(\)\)/);
});
