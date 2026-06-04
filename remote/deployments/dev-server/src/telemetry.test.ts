import { describe, it } from 'node:test';
import { strict as assert } from 'node:assert';

import { applyRequestContextAttributes, withSpan, type TelemetrySpan } from './telemetry.js';
import {
  readErrorRequestContext,
  runWithRequestContext,
  setContextField,
  setContextExtra,
  snapshotRequestContext,
} from './request-context.js';

type AttrValue = string | number | boolean | undefined;

function makeRecordingSpan(): { span: TelemetrySpan; attrs: Map<string, AttrValue>; exceptions: Error[]; status: { code: 'ok' | 'error'; message?: string } } {
  const attrs = new Map<string, AttrValue>();
  const exceptions: Error[] = [];
  let status: { code: 'ok' | 'error'; message?: string } = { code: 'ok' };
  const span: TelemetrySpan = {
    setAttribute(key, value) {
      if (value === undefined) return;
      attrs.set(key, value);
    },
    recordException(err) {
      exceptions.push(err);
    },
    setStatus(s) {
      status = s;
    },
  };
  return {
    span,
    attrs,
    exceptions,
    get status() {
      return status;
    },
  } as ReturnType<typeof makeRecordingSpan>;
}

describe('telemetry: applyRequestContextAttributes', () => {
  it('is a no-op when ctx is null', () => {
    const rec = makeRecordingSpan();
    applyRequestContextAttributes(rec.span, null);
    assert.equal(rec.attrs.size, 0);
  });

  it('stamps every known field as a dd.request.* attribute', () => {
    const rec = makeRecordingSpan();
    runWithRequestContext(
      {
        requestId: 'r-1',
        threadId: 't-1',
        taskId: 'task-1',
        userId: 'u-1',
        provider: 'claude-cli',
        method: 'POST',
        route: '/tasks',
      },
      () => {
        setContextExtra('containerPoolRequestId', 'cpr-1');
        applyRequestContextAttributes(rec.span, snapshotRequestContext());
      },
    );
    assert.equal(rec.attrs.get('dd.request.id'), 'r-1');
    assert.equal(rec.attrs.get('dd.request.route'), '/tasks');
    assert.equal(rec.attrs.get('dd.request.method'), 'POST');
    assert.equal(rec.attrs.get('dd.request.thread_id'), 't-1');
    assert.equal(rec.attrs.get('dd.request.task_id'), 'task-1');
    assert.equal(rec.attrs.get('dd.request.user_id'), 'u-1');
    assert.equal(rec.attrs.get('dd.request.provider'), 'claude-cli');
    assert.equal(rec.attrs.get('dd.request.extra.containerPoolRequestId'), 'cpr-1');
  });
});

describe('telemetry: withSpan + ALS integration', () => {
  it('seeds request-context attributes from the active ALS store', async () => {
    let inside: { id?: AttrValue; thread?: AttrValue; task?: AttrValue } | null = null;
    await runWithRequestContext(
      { requestId: 'span-req-1', threadId: 'thread-1', method: 'POST', route: '/tasks' },
      async () => {
        await withSpan('test.span', { 'custom.attr': 'present' }, async (span) => {
          // Span impl is internal; we can't peek its attrs from here.
          // Instead, verify ALS context is still active inside withSpan
          // (so applyRequestContextAttributes would have a snapshot).
          const ctx = snapshotRequestContext();
          inside = {
            id: ctx?.requestId,
            thread: ctx?.threadId,
            task: ctx?.taskId,
          };
          // setAttribute is callable from inside work().
          span.setAttribute('observation', 'inside');
        });
      },
    );
    assert.deepEqual(inside, {
      id: 'span-req-1',
      thread: 'thread-1',
      task: undefined,
    });
  });

  it('picks up late-bound context fields set inside the handler (success path)', async () => {
    const result = await runWithRequestContext({ requestId: 'span-req-2' }, async () => {
      return withSpan('test.late-bind', {}, async () => {
        setContextField('taskId', 'task-late');
        setContextField('provider', 'claude-cli');
        const ctx = snapshotRequestContext();
        return ctx?.taskId ?? 'missing';
      });
    });
    assert.equal(result, 'task-late');
  });

  it('annotates errors thrown inside withSpan with the request context (error path)', async () => {
    let captured: unknown = null;
    try {
      await runWithRequestContext(
        { requestId: 'span-req-3', threadId: 'thread-err', taskId: 'task-err' },
        async () => {
          await withSpan('test.error', {}, async () => {
            throw new Error('handler boom');
          });
        },
      );
    } catch (err) {
      captured = err;
    }
    assert.ok(captured instanceof Error, 'should throw an Error');
    assert.equal((captured as Error).message, 'handler boom');
    const tagged = readErrorRequestContext(captured);
    assert.ok(tagged, 'requestContext should be attached by withSpan via annotateError');
    assert.equal(tagged!.requestId, 'span-req-3');
    assert.equal(tagged!.threadId, 'thread-err');
    assert.equal(tagged!.taskId, 'task-err');
  });

  it('withSpan called outside any request context still works (no attributes seeded)', async () => {
    const result = await withSpan('test.no-context', {}, async () => 'ok');
    assert.equal(result, 'ok');
  });
});
