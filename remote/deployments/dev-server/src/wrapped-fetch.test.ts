import { describe, it, before, after } from 'node:test';
import { strict as assert } from 'node:assert';
import { createServer, type Server, type IncomingMessage } from 'node:http';
import { AddressInfo } from 'node:net';

import { contextFetch } from './wrapped-fetch.js';
import {
  readErrorRequestContext,
  runWithRequestContext,
} from './request-context.js';

type Captured = {
  url: string;
  method: string | undefined;
  headers: IncomingMessage['headers'];
};

let server: Server;
let baseUrl: string;
let captured: Captured[] = [];

before(async () => {
  server = createServer((req, res) => {
    captured.push({ url: req.url ?? '', method: req.method, headers: req.headers });
    if (req.url === '/echo-error') {
      res.statusCode = 503;
      res.end('upstream failure');
      return;
    }
    res.statusCode = 200;
    res.setHeader('content-type', 'application/json');
    res.end(JSON.stringify({ ok: true }));
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', () => resolve()));
  const addr = server.address() as AddressInfo;
  baseUrl = `http://127.0.0.1:${addr.port}`;
});

after(async () => {
  await new Promise<void>((resolve, reject) =>
    server.close((err) => (err ? reject(err) : resolve())),
  );
});

describe('contextFetch: header propagation', () => {
  it('injects x-request-id from the active ALS context', async () => {
    captured = [];
    await runWithRequestContext({ requestId: 'req-abc' }, async () => {
      const res = await contextFetch(`${baseUrl}/`);
      assert.equal(res.status, 200);
    });
    assert.equal(captured.length, 1);
    assert.equal(captured[0]!.headers['x-request-id'], 'req-abc');
  });

  it('injects x-dd-thread-id and x-dd-task-id when present in context', async () => {
    captured = [];
    await runWithRequestContext(
      { requestId: 'req-with-ids', threadId: 'thread-1', taskId: 'task-1' },
      async () => {
        await contextFetch(`${baseUrl}/`);
      },
    );
    assert.equal(captured[0]!.headers['x-dd-thread-id'], 'thread-1');
    assert.equal(captured[0]!.headers['x-dd-task-id'], 'task-1');
  });

  it('does not override a caller-supplied x-request-id', async () => {
    captured = [];
    await runWithRequestContext({ requestId: 'req-context' }, async () => {
      await contextFetch(`${baseUrl}/`, {
        headers: { 'x-request-id': 'caller-supplied' },
      });
    });
    assert.equal(captured[0]!.headers['x-request-id'], 'caller-supplied');
  });

  it('skips header injection when called outside any request context', async () => {
    captured = [];
    await contextFetch(`${baseUrl}/`);
    assert.equal(captured.length, 1);
    assert.equal(captured[0]!.headers['x-request-id'], undefined);
    assert.equal(captured[0]!.headers['x-dd-thread-id'], undefined);
    assert.equal(captured[0]!.headers['x-dd-task-id'], undefined);
  });

  it('preserves a caller-supplied custom header alongside injected ones', async () => {
    captured = [];
    await runWithRequestContext({ requestId: 'req-keep' }, async () => {
      await contextFetch(`${baseUrl}/`, {
        method: 'POST',
        headers: { 'x-custom': 'one', 'content-type': 'application/json' },
        body: JSON.stringify({ ok: true }),
      });
    });
    assert.equal(captured[0]!.method, 'POST');
    assert.equal(captured[0]!.headers['x-custom'], 'one');
    assert.equal(captured[0]!.headers['x-request-id'], 'req-keep');
  });

  it('returns non-2xx responses without throwing (call site decides)', async () => {
    captured = [];
    await runWithRequestContext({ requestId: 'req-503' }, async () => {
      const res = await contextFetch(`${baseUrl}/echo-error`);
      assert.equal(res.status, 503);
      assert.equal(await res.text(), 'upstream failure');
    });
  });
});

describe('contextFetch: error annotation', () => {
  it('rethrows network errors annotated with the request context', async () => {
    let captured: unknown = null;
    try {
      await runWithRequestContext({ requestId: 'err-req', taskId: 'err-task' }, async () => {
        // Port 1 on localhost — guaranteed connection refused.
        await contextFetch('http://127.0.0.1:1/');
      });
    } catch (err) {
      captured = err;
    }
    assert.ok(captured instanceof Error, 'should throw an Error');
    const tagged = readErrorRequestContext(captured);
    assert.ok(tagged, 'requestContext should be attached to thrown error');
    assert.equal(tagged!.requestId, 'err-req');
    assert.equal(tagged!.taskId, 'err-task');
  });

  it('errors thrown outside a context are not annotated', async () => {
    let captured: unknown = null;
    try {
      await contextFetch('http://127.0.0.1:1/');
    } catch (err) {
      captured = err;
    }
    assert.ok(captured instanceof Error);
    assert.equal(readErrorRequestContext(captured), null);
  });
});
