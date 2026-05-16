import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { chromium } from 'playwright';

const baseUrl = (process.env.REMOTE_DEV_BASE_URL ?? 'http://54.91.17.58').replace(/\/+$/, '');
const provider = process.env.REMOTE_DEV_ECHO_PROVIDER ?? 'claude-sdk';
const prompt = process.env.REMOTE_DEV_ECHO_PROMPT ?? "please echo back 'hello'";
const expected = process.env.REMOTE_DEV_ECHO_EXPECTED ?? 'hello';
const timeoutMs = Number(process.env.REMOTE_DEV_ECHO_TIMEOUT_MS ?? 420_000);

console.log(`[ui-agent-echo] target=${baseUrl}/agents/tasks provider=${provider}`);

const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ ignoreHTTPSErrors: true });
const page = await context.newPage();

try {
  const response = await page.goto(`${baseUrl}/agents/tasks`, {
    waitUntil: 'domcontentloaded',
    timeout: 60_000,
  });
  assert.ok(response, 'expected /agents/tasks response');
  assert.equal(response.status(), 200);

  const threadId = randomUUID();
  const taskId = randomUUID();

  await page.locator('#chat-thread-id').fill(threadId);
  await page.locator('#chat-task-id').fill(taskId);
  await page.locator('#chat-provider').selectOption(provider);
  await page.locator('#chat-prompt').fill(prompt);
  await page.locator('#send-chat').click();

  await page.waitForFunction(
    ({ expectedText }) => {
      const text = document.querySelector('#chat-stream')?.textContent?.toLowerCase() ?? '';
      return text.includes(expectedText.toLowerCase()) || text.includes('dispatch failed');
    },
    { expectedText: expected },
    { timeout: timeoutMs },
  );

  const streamText = await page.locator('#chat-stream').innerText();
  assert.doesNotMatch(streamText, /dispatch failed/i);
  assert.match(streamText.toLowerCase(), new RegExp(expected.toLowerCase()));
  assert.match(
    streamText,
    new RegExp(taskId.slice(0, 8)),
    'expected stream to include task context',
  );
  console.log(`[ui-agent-echo] thread=${threadId} task=${taskId}`);
  console.log('[ui-agent-echo] PASS');
} finally {
  await context.close();
  await browser.close();
}
