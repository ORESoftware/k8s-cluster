// Type-only sanity checks for the discriminated union — the property the
// upstream live-mutex library lacks. Run via `pnpm --dir clients/ts test`.

import { describe, test } from "node:test";
import assert from "node:assert/strict";
import { NetworkMutexClient } from "../src/client.ts";
import { assertNever, type LockRequest, type Request, type Response } from "../src/protocol.ts";

describe("protocol exhaustiveness", () => {
  test("dispatch covers every Response variant", () => {
    function dispatch(resp: Response): string {
      switch (resp.type) {
        case "version": return "version";
        case "auth": return "auth";
        case "lock": return "lock";
        case "compositeLock": return "compositeLock";
        case "unlock": return "unlock";
        case "registerReadResult": return "registerReadResult";
        case "registerWriteResult": return "registerWriteResult";
        case "endReadResult": return "endReadResult";
        case "endWriteResult": return "endWriteResult";
        case "lockInfo": return "lockInfo";
        case "lsResult": return "lsResult";
        case "reelection": return "reelection";
        case "error": return "error";
        case "ok": return "ok";
        default: return assertNever(resp);
      }
    }
    const ok: Response = { type: "ok", uuid: "x" };
    assert.equal(dispatch(ok), "ok");
  });
});

describe("wait/no-wait lock requests", () => {
  test("LockRequest carries explicit wait true/false on the wire", () => {
    const blocking: LockRequest = {
      type: "lock",
      uuid: "u-block",
      keys: ["a", "b"],
      ttl: 1000,
      wait: true,
    };
    const noWait: LockRequest = {
      type: "lock",
      uuid: "u-nowait",
      keys: ["a", "b"],
      ttl: 1000,
      wait: false,
    };
    const omitted: LockRequest = {
      type: "lock",
      uuid: "u-omit",
      keys: ["a", "b"],
      ttl: 1000,
    };

    assert.equal(JSON.parse(JSON.stringify(blocking)).wait, true);
    assert.equal(JSON.parse(JSON.stringify(noWait)).wait, false);
    assert.equal("wait" in JSON.parse(JSON.stringify(omitted)), false);
  });

  test("tryAcquireMany sends wait:false and returns null on contention", async () => {
    const client = new NetworkMutexClient();
    const sent: Request[] = [];
    (client as unknown as { send(req: Request): Promise<Response> }).send = async (req) => {
      sent.push(req);
      assert.equal(req.type, "lock");
      return {
        type: "compositeLock",
        uuid: req.uuid,
        keys: (req as LockRequest).keys ?? [],
        acquired: false,
      };
    };

    const handle = await client.tryAcquireMany(["a", "b"]);

    assert.equal(handle, null);
    assert.equal(sent.length, 1);
    assert.equal((sent[0] as LockRequest).wait, false);
  });

  test("acquireMany sends wait:true and waits for the terminal grant", async () => {
    const client = new NetworkMutexClient();
    const sent: Request[] = [];
    (client as unknown as { awaitGrant(req: Request): Promise<Response> }).awaitGrant = async (req) => {
      sent.push(req);
      assert.equal(req.type, "lock");
      return {
        type: "compositeLock",
        uuid: req.uuid,
        keys: (req as LockRequest).keys ?? [],
        acquired: true,
        lockUuid: "L-1",
        fencingTokens: { a: 1, b: 2 },
      };
    };

    const handle = await client.acquireMany(["a", "b"], { ttlMs: 1000, waitMs: 1000 });

    assert.equal(handle.lockUuid, "L-1");
    assert.equal(sent.length, 1);
    assert.equal((sent[0] as LockRequest).wait, true);
  });
});
