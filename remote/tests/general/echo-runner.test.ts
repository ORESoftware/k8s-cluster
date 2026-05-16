import assert from 'node:assert/strict';
import { resolve } from 'node:path';
import test from 'node:test';

const repoRoot = resolve(process.cwd(), '..', '..');
const { extractEchoText } = await import(
  resolve(repoRoot, 'remote/dev-server/src/agents/echo.ts')
);

test('echo runner extracts the current user prompt from context-wrapped tasks', () => {
  const wrapped = `You are continuing remote development thread example.

<previous_thread_context>
prompt: please echo back stale context
</previous_thread_context>

<current_user_prompt>
please echo back hello
</current_user_prompt>`;

  assert.equal(extractEchoText(wrapped), 'hello');
});

test('echo runner supports common echo phrasing', () => {
  assert.equal(extractEchoText("please echo back 'hello'"), 'hello');
  assert.equal(extractEchoText('echo hello back'), 'hello');
  assert.equal(extractEchoText('echo hello'), 'hello');
});

test('echo runner fallback still uses the current user prompt instead of stale thread context', () => {
  const wrapped = `You are continuing remote development thread example.

<previous_thread_context>
prompt: echo back stale context
</previous_thread_context>

<current_user_prompt>
just show the current prompt
</current_user_prompt>`;

  assert.equal(extractEchoText(wrapped), 'Echo: just show the current prompt');
});
