import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const generatorPath = path.join(packageRoot, 'src', 'generate.mjs');

test('generated outputs are up to date with schema source', () => {
  execFileSync(process.execPath, [generatorPath, '--check'], {
    cwd: packageRoot,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
});

test('typescript output exposes the agent task queue envelope', async () => {
  const ts = await readFile(path.join(packageRoot, 'generated', 'typescript', 'index.ts'), 'utf8');

  assert.match(ts, /export type AgentTaskQueueMessage = \{/);
  assert.match(ts, /threadId: string;/);
  assert.match(ts, /taskId: string;/);
  assert.match(ts, /containerPoolDispatch\?: boolean;/);
});

test('rust output exposes the agent task queue envelope', async () => {
  const rs = await readFile(path.join(packageRoot, 'generated', 'rust', 'src', 'lib.rs'), 'utf8');

  assert.match(rs, /pub struct AgentTaskQueueMessage \{/);
  assert.match(rs, /pub thread_id: String,/);
  assert.match(rs, /pub task_id: String,/);
  assert.match(rs, /pub container_pool_dispatch: Option<bool>,/);
});
