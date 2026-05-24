// Type-only sanity checks for the discriminated union — the property the
// upstream live-mutex library lacks. Run via `pnpm --dir clients/ts test`.

import { describe, test } from "node:test";
import assert from "node:assert/strict";
import { assertNever, type Response } from "../src/protocol.ts";

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
