import { describe, it } from 'node:test';
import { strict as assert } from 'node:assert';
import { setTimeout as delay } from 'node:timers/promises';
import { spawn } from 'node:child_process';
import { EventEmitter } from 'node:events';

import {
  annotateError,
  bindToCurrentContext,
  getRequestContext,
  readErrorRequestContext,
  runWithRequestContext,
  setContextField,
  setContextExtra,
  snapshotRequestContext,
} from './request-context.js';

describe('request-context: AsyncLocalStorage core', () => {
  it('seeds defaults (requestId + startedAt) when not provided', () => {
    runWithRequestContext({}, () => {
      const ctx = getRequestContext();
      assert.ok(ctx, 'ctx should exist inside runWithRequestContext');
      assert.match(ctx!.requestId, /^[0-9a-f-]{36}$/);
      assert.ok(typeof ctx!.startedAt === 'number');
      assert.deepEqual(ctx!.extra, {});
    });
  });

  it('returns undefined outside any context', () => {
    assert.equal(getRequestContext(), undefined);
  });

  it('seed values override defaults', () => {
    runWithRequestContext(
      { requestId: 'fixed-id', threadId: 't-1', method: 'POST', route: '/tasks' },
      () => {
        const ctx = getRequestContext()!;
        assert.equal(ctx.requestId, 'fixed-id');
        assert.equal(ctx.threadId, 't-1');
        assert.equal(ctx.method, 'POST');
        assert.equal(ctx.route, '/tasks');
      },
    );
  });

  it('setContextField mutates the live store', () => {
    runWithRequestContext({ requestId: 'r-1' }, () => {
      setContextField('taskId', 'task-42');
      setContextField('provider', 'claude-cli');
      const ctx = getRequestContext()!;
      assert.equal(ctx.taskId, 'task-42');
      assert.equal(ctx.provider, 'claude-cli');
    });
  });

  it('setContextExtra mutates extra bag', () => {
    runWithRequestContext({ requestId: 'r-1' }, () => {
      setContextExtra('containerPoolRequestId', 'cpr-1');
      const ctx = getRequestContext()!;
      assert.equal(ctx.extra.containerPoolRequestId, 'cpr-1');
    });
  });

  it('setContextField is a no-op outside a context', () => {
    setContextField('taskId', 'should-not-explode');
    assert.equal(getRequestContext(), undefined);
  });

  it('context isolates between concurrent runs', async () => {
    const observed: Array<string | undefined> = [];
    const run = async (id: string) =>
      runWithRequestContext({ requestId: id }, async () => {
        await delay(5);
        observed.push(getRequestContext()?.requestId);
      });
    await Promise.all([run('a'), run('b'), run('c')]);
    observed.sort();
    assert.deepEqual(observed, ['a', 'b', 'c']);
  });
});

describe('request-context: propagation through async primitives', () => {
  it('survives a single await', async () => {
    await runWithRequestContext({ requestId: 'await-1' }, async () => {
      await delay(1);
      assert.equal(getRequestContext()?.requestId, 'await-1');
    });
  });

  it('survives chained promises', async () => {
    await runWithRequestContext({ requestId: 'chain-1' }, async () => {
      await Promise.resolve();
      await delay(1);
      await Promise.resolve();
      assert.equal(getRequestContext()?.requestId, 'chain-1');
    });
  });

  it('survives setTimeout / setImmediate', async () => {
    await runWithRequestContext({ requestId: 'timer-1' }, async () => {
      await new Promise<void>((resolve) =>
        setTimeout(() => {
          assert.equal(getRequestContext()?.requestId, 'timer-1');
          setImmediate(() => {
            assert.equal(getRequestContext()?.requestId, 'timer-1');
            resolve();
          });
        }, 1),
      );
    });
  });
});

describe('request-context: annotateError + readErrorRequestContext', () => {
  it('annotateError attaches a non-enumerable requestContext snapshot', () => {
    runWithRequestContext({ requestId: 'err-1', taskId: 'task-x' }, () => {
      const err = new Error('boom');
      const annotated = annotateError(err);
      const recovered = readErrorRequestContext(annotated);
      assert.ok(recovered);
      assert.equal(recovered!.requestId, 'err-1');
      assert.equal(recovered!.taskId, 'task-x');
      assert.equal(annotated, err, 'annotation is in-place');
      assert.equal(
        JSON.stringify(annotated),
        '{}',
        'requestContext should not leak into JSON.stringify(err)',
      );
    });
  });

  it('annotateError converts non-Error into Error', () => {
    runWithRequestContext({ requestId: 'err-2' }, () => {
      const annotated = annotateError('not an error');
      assert.ok(annotated instanceof Error);
      assert.equal(annotated.message, 'not an error');
      assert.equal(readErrorRequestContext(annotated)?.requestId, 'err-2');
    });
  });

  it('annotateError is a no-op when called outside a context', () => {
    const err = new Error('outside');
    const annotated = annotateError(err);
    assert.equal(readErrorRequestContext(annotated), null);
  });

  it('annotateError does not overwrite an existing tag', () => {
    const err = new Error('boom');
    runWithRequestContext({ requestId: 'first' }, () => annotateError(err));
    runWithRequestContext({ requestId: 'second' }, () => annotateError(err));
    assert.equal(readErrorRequestContext(err)?.requestId, 'first');
  });

  it('snapshotRequestContext returns null outside any context', () => {
    assert.equal(snapshotRequestContext(), null);
  });
});

describe('request-context: bindToCurrentContext (AsyncResource.bind)', () => {
  it('preserves context when callback fires from outside the run scope', async () => {
    let bound: (() => string | undefined) | null = null;

    runWithRequestContext({ requestId: 'bound-1' }, () => {
      bound = bindToCurrentContext(() => getRequestContext()?.requestId);
    });

    assert.equal(getRequestContext(), undefined);
    assert.equal(bound!(), 'bound-1');
  });

  it('plain (unbound) callbacks lose context when invoked from outside', async () => {
    let unbound: (() => string | undefined) | null = null;

    runWithRequestContext({ requestId: 'unbound-1' }, () => {
      unbound = () => getRequestContext()?.requestId;
    });

    assert.equal(unbound!(), undefined);
  });
});

describe('request-context: EventEmitter listener binding (per-listener, no monkeypatching)', () => {
  it('bindToCurrentContext on an EventEmitter listener preserves context across emit-from-outside', async () => {
    const emitter = new EventEmitter();
    let received: string | undefined;

    runWithRequestContext({ requestId: 'emitter-1' }, () => {
      emitter.on(
        'data',
        bindToCurrentContext(() => {
          received = getRequestContext()?.requestId;
        }),
      );
    });

    assert.equal(getRequestContext(), undefined, 'listener fires from outside the run');
    emitter.emit('data');
    assert.equal(received, 'emitter-1');
  });

  it('plain unbound listener loses context when emit happens outside the run scope', async () => {
    const emitter = new EventEmitter();
    let received: string | undefined;

    runWithRequestContext({ requestId: 'emitter-2' }, () => {
      emitter.on('data', () => {
        received = getRequestContext()?.requestId;
      });
    });

    emitter.emit('data');
    assert.equal(received, undefined, 'no helper, no context preservation');
  });
});

describe('request-context: child_process.spawn callbacks', () => {
  it('listeners attached inside the run scope keep ALS context', async () => {
    const observed: Record<string, string | undefined> = {};

    await runWithRequestContext({ requestId: 'spawn-1', taskId: 'task-spawn' }, async () => {
      const child = spawn(process.execPath, ['-e', 'console.log("hello"); process.exit(0)']);

      await new Promise<void>((resolve, reject) => {
        child.stdout.on('data', () => {
          observed.data = getRequestContext()?.requestId;
        });
        child.on('exit', (code) => {
          observed.exit = getRequestContext()?.requestId;
          if (code === 0) resolve();
          else reject(new Error(`unexpected exit code ${code}`));
        });
        child.on('error', reject);
      });
    });

    assert.equal(observed.data, 'spawn-1', 'stdout data listener kept ALS context');
    assert.equal(observed.exit, 'spawn-1', 'exit listener kept ALS context');
  });

  it('bindToCurrentContext keeps context for a listener attached in a later tick', async () => {
    const observed: Record<string, string | undefined> = {};

    let child: ReturnType<typeof spawn> | null = null;
    let boundExitListener: ((code: number | null) => void) | null = null;

    await runWithRequestContext({ requestId: 'spawn-bind' }, async () => {
      child = spawn(process.execPath, ['-e', 'setTimeout(() => process.exit(0), 20)']);
      // Capture context NOW; attach later from outside the run scope.
      boundExitListener = bindToCurrentContext((code: number | null) => {
        observed.exit = getRequestContext()?.requestId;
        observed.code = String(code);
      });
    });

    await new Promise<void>((resolve, reject) => {
      const c = child!;
      c.on('exit', (code) => {
        boundExitListener!(code);
        if (code === 0) resolve();
        else reject(new Error(`unexpected exit code ${code}`));
      });
      c.on('error', reject);
    });

    assert.equal(observed.exit, 'spawn-bind');
    assert.equal(getRequestContext(), undefined, 'outer scope has no ALS context after the run');
  });
});
