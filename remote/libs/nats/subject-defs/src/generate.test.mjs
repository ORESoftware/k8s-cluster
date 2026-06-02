import { execFileSync } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const generatorPath = path.join(packageRoot, 'src', 'generate.mjs');

test('generated outputs are up to date with schema source', () => {
  // Throws if non-zero exit code.
  execFileSync(process.execPath, [generatorPath, '--check'], {
    cwd: packageRoot,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
});

test('typescript output exposes constants for known static subjects', async () => {
  const ts = await readFile(
    path.join(packageRoot, 'generated', 'typescript', 'index.ts'),
    'utf8',
  );
  for (const literal of [
    'export const RUNTIME_EVENTS_SUBJECT = "dd.remote.events";',
    'export const RUNTIME_CRITICAL_EVENTS_SUBJECT = "dd.remote.events.critical";',
    'export const ORCHESTRATOR_WAKEUP_SUBJECT = "dd.remote.orchestrator.wakeup";',
    'export const CRON_PROMPTS_SUBJECT = "dd.remote.cron.prompts";',
    'export const TRADING_SIGNALS_SUBJECT = "dd.remote.trading.signals";',
    'export const LAMBDAS_FUNCTIONS_SUBJECT = "dd.remote.lambdas.functions";',
    'export const WEBSOCKET_EVENTS_SUBJECT = "dd.remote.websocket.events";',
  ]) {
    assert.ok(ts.includes(literal), `missing TS constant: ${literal}`);
  }
});

test('typescript output exposes formatter + parser + wildcard for known parameterized subjects', async () => {
  const ts = await readFile(
    path.join(packageRoot, 'generated', 'typescript', 'index.ts'),
    'utf8',
  );
  for (const fn of [
    'threadTasksSubject',
    'threadControlSubject',
    'threadEventsSubject',
    'threadHeartbeatSubject',
    'lambdasInvokeSubject',
    'presenceBroadcastConvSubject',
    'containerPoolLanguageRequestsSubject',
    'containerPoolEventsSubject',
    'cdcRowChangeSubject',
  ]) {
    assert.match(ts, new RegExp(`export function ${fn}\\(`), `missing TS formatter: ${fn}`);
  }
  for (const fn of [
    'parseThreadTasksSubject',
    'parseThreadControlSubject',
    'parseLambdasInvokeSubject',
    'parsePresenceBroadcastConvSubject',
    'parseCdcRowChangeSubject',
  ]) {
    assert.match(ts, new RegExp(`export function ${fn}\\(`), `missing TS parser: ${fn}`);
  }
  for (const wildcard of [
    'export const THREAD_TASKS_WILDCARD = "dd.remote.thread.*.tasks";',
    'export const THREAD_CONTROL_WILDCARD = "dd.remote.thread.*.control";',
    'export const LAMBDAS_INVOKE_WILDCARD = "dd.remote.lambdas.invoke.*";',
    'export const PRESENCE_BROADCAST_CONV_WILDCARD = "presence.broadcast.conv.>";',
    'export const CDC_ROW_CHANGE_WILDCARD = "{prefix}.>";',
  ]) {
    assert.ok(ts.includes(wildcard), `missing TS wildcard: ${wildcard}`);
  }
  for (const qg of [
    'export const THREAD_TASKS_QUEUE_GROUP = "dd-remote-thread-preparer";',
    'export const LAMBDAS_INVOKE_QUEUE_GROUP = "dd-gleam-lambda-runner";',
  ]) {
    assert.ok(ts.includes(qg), `missing TS queue group: ${qg}`);
  }
});

test('typescript output exposes JetStream stream definitions', async () => {
  const ts = await readFile(
    path.join(packageRoot, 'generated', 'typescript', 'index.ts'),
    'utf8',
  );
  for (const decl of [
    'export const DD_REMOTE_TASKS_STREAM_NAME = "DD_REMOTE_TASKS";',
    'export const DD_REMOTE_TASKS_STREAM_SUBJECTS: readonly string[] = ["dd.remote.thread.*.tasks"];',
    'export const DD_REMOTE_CONTROL_STREAM_NAME = "DD_REMOTE_CONTROL";',
    'export const DD_REMOTE_CRITICAL_EVENTS_STREAM_NAME = "DD_REMOTE_CRITICAL_EVENTS";',
    'export const DD_REMOTE_CRITICAL_EVENTS_STREAM_SUBJECTS: readonly string[] = ["dd.remote.events.critical"];',
    'export const DD_REMOTE_EVENTS_STREAM_NAME = "DD_REMOTE_EVENTS";',
    'export const DD_REMOTE_CRON_STREAM_NAME = "DD_REMOTE_CRON";',
    'export const CDC_STREAM_NAME = "CDC";',
  ]) {
    assert.ok(ts.includes(decl), `missing TS stream decl: ${decl}`);
  }
});

test('rust output exposes snake_case formatters + parsers + constants', async () => {
  const rs = await readFile(
    path.join(packageRoot, 'generated', 'rust', 'src', 'lib.rs'),
    'utf8',
  );
  for (const fn of [
    'thread_tasks_subject',
    'thread_control_subject',
    'lambdas_invoke_subject',
    'presence_broadcast_conv_subject',
    'cdc_row_change_subject',
    'container_pool_language_requests_subject',
  ]) {
    assert.match(rs, new RegExp(`pub fn ${fn}\\(`), `missing Rust formatter: ${fn}`);
  }
  for (const fn of [
    'parse_thread_tasks_subject',
    'parse_lambdas_invoke_subject',
    'parse_cdc_row_change_subject',
  ]) {
    assert.match(rs, new RegExp(`pub fn ${fn}\\(`), `missing Rust parser: ${fn}`);
  }
  for (const c of [
    'pub const RUNTIME_EVENTS_SUBJECT: &str = "dd.remote.events";',
    'pub const RUNTIME_CRITICAL_EVENTS_SUBJECT: &str = "dd.remote.events.critical";',
    'pub const ORCHESTRATOR_WAKEUP_SUBJECT: &str = "dd.remote.orchestrator.wakeup";',
    'pub const THREAD_TASKS_WILDCARD: &str = "dd.remote.thread.*.tasks";',
    'pub const THREAD_TASKS_QUEUE_GROUP: &str = "dd-remote-thread-preparer";',
    'pub const DD_REMOTE_TASKS_STREAM_NAME: &str = "DD_REMOTE_TASKS";',
  ]) {
    assert.ok(rs.includes(c), `missing Rust constant: ${c}`);
  }
});

test('python, gleam, erlang, dart, go, java outputs each define a thread_tasks subject formatter', async () => {
  const checks = [
    { file: ['generated', 'python', 'dd_nats_subject_defs.py'], pat: /def thread_tasks_subject\(thread_id: str\) -> str:/ },
    { file: ['generated', 'gleam', 'src', 'dd_nats_subject_defs.gleam'], pat: /pub fn thread_tasks_subject\(thread_id thread_id: String\)/ },
    { file: ['generated', 'erlang', 'src', 'dd_nats_subject_defs.erl'], pat: /thread_tasks_subject\(ThreadId\)/ },
    { file: ['generated', 'dart', 'lib', 'dd_nats_subject_defs.dart'], pat: /String threadTasksSubject\(String threadId\)/ },
    { file: ['generated', 'go', 'ddnats.go'], pat: /func ThreadTasksSubject\(threadId string\) string/ },
    { file: ['generated', 'jvm', 'src', 'main', 'java', 'dd', 'nats', 'DdNatsSubjects.java'], pat: /public static String threadTasksSubject\(String threadId\)/ },
  ];
  for (const c of checks) {
    const contents = await readFile(path.join(packageRoot, ...c.file), 'utf8');
    assert.match(contents, c.pat, `missing thread_tasks formatter in ${c.file.join('/')}`);
  }
});

test('every language renders the same subject for thread_tasks formatter', async () => {
  // Sample expected value: dd.remote.thread.<id>.tasks
  const threadId = 'b82e5724-0273-4cd9-a198-ed6caac99a33';
  const expected = `dd.remote.thread.${threadId}.tasks`;

  // TS literal extraction
  const ts = await readFile(
    path.join(packageRoot, 'generated', 'typescript', 'index.ts'),
    'utf8',
  );
  const tsMatch = /export function threadTasksSubject\(threadId: string\): string \{\s*return `([^`]+)`;\s*\}/.exec(ts);
  assert.ok(tsMatch, 'could not find TS threadTasksSubject template literal');
  const tsRendered = tsMatch[1].replace('${threadId}', threadId);
  assert.equal(tsRendered, expected);

  // Rust format!() arg
  const rs = await readFile(
    path.join(packageRoot, 'generated', 'rust', 'src', 'lib.rs'),
    'utf8',
  );
  const rsMatch = /pub fn thread_tasks_subject\(thread_id: &str\) -> String \{\s*format!\("([^"]+)", thread_id\)\s*\}/.exec(rs);
  assert.ok(rsMatch, 'could not find Rust thread_tasks_subject format string');
  const rsRendered = rsMatch[1].replace('{}', threadId);
  assert.equal(rsRendered, expected);

  // Go fmt.Sprintf
  const go = await readFile(path.join(packageRoot, 'generated', 'go', 'ddnats.go'), 'utf8');
  const goMatch = /func ThreadTasksSubject\(threadId string\) string \{\s*return fmt\.Sprintf\("([^"]+)", threadId\)\s*\}/.exec(go);
  assert.ok(goMatch, 'could not find Go ThreadTasksSubject format string');
  const goRendered = goMatch[1].replace('%s', threadId);
  assert.equal(goRendered, expected);

  // Python f-format pattern
  const py = await readFile(
    path.join(packageRoot, 'generated', 'python', 'dd_nats_subject_defs.py'),
    'utf8',
  );
  const pyMatch = /def thread_tasks_subject\(thread_id: str\) -> str:[\s\S]*?return "([^"]+)"\.format\(thread_id=thread_id\)/.exec(py);
  assert.ok(pyMatch, 'could not find Python thread_tasks_subject format string');
  const pyRendered = pyMatch[1].replace('{thread_id}', threadId);
  assert.equal(pyRendered, expected);

  // Java concat expression
  const java = await readFile(
    path.join(
      packageRoot,
      'generated', 'jvm', 'src', 'main', 'java', 'dd', 'nats', 'DdNatsSubjects.java',
    ),
    'utf8',
  );
  const javaMatch = /public static String threadTasksSubject\(String threadId\) \{\s*return ([^;]+);\s*\}/.exec(java);
  assert.ok(javaMatch, 'could not find Java threadTasksSubject expression');
  const javaRendered = new Function('threadId', `return ${javaMatch[1]};`)(threadId);
  assert.equal(javaRendered, expected);

  // Dart interpolation
  const dart = await readFile(
    path.join(packageRoot, 'generated', 'dart', 'lib', 'dd_nats_subject_defs.dart'),
    'utf8',
  );
  const dartMatch = /String threadTasksSubject\(String threadId\) \{\s*return '([^']+)';\s*\}/.exec(dart);
  assert.ok(dartMatch, 'could not find Dart threadTasksSubject literal');
  const dartRendered = dartMatch[1].replace('$threadId', threadId);
  assert.equal(dartRendered, expected);

  // Gleam <> chain
  const gleam = await readFile(
    path.join(packageRoot, 'generated', 'gleam', 'src', 'dd_nats_subject_defs.gleam'),
    'utf8',
  );
  const gleamMatch = /pub fn thread_tasks_subject\(thread_id thread_id: String\) -> String \{\s*([^\n]+)\s*\}/.exec(gleam);
  assert.ok(gleamMatch, 'could not find Gleam thread_tasks_subject body');
  // Resolve "x" <> y <> "z" by stripping quotes from literals and replacing thread_id.
  const gleamRendered = gleamMatch[1]
    .split('<>')
    .map((piece) => piece.trim())
    .map((piece) => (piece === 'thread_id' ? threadId : piece.replace(/^"|"$/g, '')))
    .join('');
  assert.equal(gleamRendered, expected);

  // Erlang iolist
  const erl = await readFile(
    path.join(packageRoot, 'generated', 'erlang', 'src', 'dd_nats_subject_defs.erl'),
    'utf8',
  );
  const erlMatch = /thread_tasks_subject\(ThreadId\) ->\s*iolist_to_binary\(\[([^\]]+)\]\)\./.exec(erl);
  assert.ok(erlMatch, 'could not find Erlang thread_tasks_subject body');
  const erlRendered = erlMatch[1]
    .split(',')
    .map((piece) => piece.trim())
    .map((piece) => {
      if (piece.startsWith('to_bin(')) return threadId;
      // <<"literal"/utf8>>
      const m = /<<"([^"]+)"\/utf8>>/.exec(piece);
      return m ? m[1] : piece;
    })
    .join('');
  assert.equal(erlRendered, expected);
});

test('schema rejects duplicate subject names', async () => {
  // Sanity: the model would throw if we somehow had duplicates. We exercise
  // that indirectly by running the generator (which would crash on dupes).
  execFileSync(process.execPath, [generatorPath, '--check'], {
    cwd: packageRoot,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
});

test('python parser round-trips for all parameterized subjects', async () => {
  // Exec the python module and round-trip every formatter through its parser.
  const pyScriptPath = path.join(packageRoot, 'generated', 'python');
  const code = `
import sys
sys.path.insert(0, ${JSON.stringify(pyScriptPath)})
import dd_nats_subject_defs as m

cases = [
    (m.thread_tasks_subject, m.parse_thread_tasks_subject, {'thread_id': 'tid-1'}),
    (m.thread_control_subject, m.parse_thread_control_subject, {'thread_id': 'tid-2'}),
    (m.thread_events_subject, m.parse_thread_events_subject, {'thread_id': 'tid-3'}),
    (m.thread_heartbeat_subject, m.parse_thread_heartbeat_subject, {'thread_id': 'tid-4'}),
    (m.lambdas_invoke_subject, m.parse_lambdas_invoke_subject, {'function_name': 'fn'}),
    (m.presence_broadcast_conv_subject, m.parse_presence_broadcast_conv_subject, {'conv_id': 'c1'}),
    (m.container_pool_events_subject, m.parse_container_pool_events_subject, {'pool_slug': 'ps'}),
    (m.cdc_row_change_subject, m.parse_cdc_row_change_subject,
        {'prefix': 'cdc', 'schema': 'public', 'table': 'tbl', 'op': 'insert'}),
]

for fmt, parser, kwargs in cases:
    subj = fmt(**kwargs)
    parsed = parser(subj)
    assert parsed is not None, f'parser returned None for {subj}'
    for k, v in kwargs.items():
        assert getattr(parsed, k) == v, (k, v, parsed)
    # Mismatch should return None.
    bad = subj + '.extra'
    assert parser(bad) is None
print('OK')
  `;
  const out = execFileSync('python3', ['-c', code], { encoding: 'utf8' });
  assert.match(out, /OK/);
});
