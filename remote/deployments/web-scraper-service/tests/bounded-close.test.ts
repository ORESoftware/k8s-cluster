import assert from 'node:assert/strict';
import test from 'node:test';

import { closeWithTimeout } from '../src/bounded-close.js';

test('reports closed when cleanup resolves', async () => {
  const problems: string[] = [];
  const outcome = await closeWithTimeout(async () => undefined, 1_000, (kind) => problems.push(kind));
  assert.equal(outcome, 'closed');
  assert.deepEqual(problems, []);
});

test('swallows a rejecting close so the original scrape error survives', async () => {
  const seen: unknown[] = [];
  const original = new Error('page.goto: net::ERR_CONNECTION_RESET');
  await assert.rejects(
    (async () => {
      try {
        throw original;
      } finally {
        await closeWithTimeout(
          async () => {
            throw new Error('Target page, context or browser has been closed');
          },
          1_000,
          (kind, error) => seen.push([kind, (error as Error).message]),
        );
      }
    })(),
    (error: unknown) => error === original,
  );
  assert.deepEqual(seen, [['failed', 'Target page, context or browser has been closed']]);
});

test('handles a synchronously throwing close', async () => {
  const kinds: string[] = [];
  const outcome = await closeWithTimeout(
    () => {
      throw new Error('boom');
    },
    1_000,
    (kind) => kinds.push(kind),
  );
  assert.equal(outcome, 'failed');
  assert.deepEqual(kinds, ['failed']);
});

test('gives up on a wedged close after the timeout', async () => {
  const kinds: string[] = [];
  const startedAt = Date.now();
  const outcome = await closeWithTimeout(
    () => new Promise(() => undefined),
    20,
    (kind) => kinds.push(kind),
  );
  assert.equal(outcome, 'timed-out');
  assert.deepEqual(kinds, ['timed-out']);
  assert.ok(Date.now() - startedAt < 1_000);
});
