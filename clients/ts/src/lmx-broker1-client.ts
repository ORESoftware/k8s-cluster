// Minimal multiplexed NDJSON client for the oresoftware/live-mutex `Broker1`.
//
// Used by the conformance cross-check as the "theirs" adapter so we can compare
// the *fencing-token-aware* TS broker against the Rust broker on equal footing
// (the published `live-mutex` npm client speaks the legacy broker and never
// surfaces fencing tokens). Speaks the Broker1 wire protocol documented in
// live-mutex/clients/PROTOCOL.md: one JSON object per line, correlated by uuid.

import { createConnection, type Socket } from "node:net";
import { randomUUID } from "node:crypto";

export interface Broker1SingleHandle {
  key: string;
  lockUuid: string;
  fencingToken: number;
}

export interface Broker1CompositeHandle {
  keys: string[];
  lockUuid: string;
  fencingTokens: Record<string, number>;
}

interface Waiter {
  resolve(msg: Record<string, unknown>): void;
  reject(err: Error): void;
  /** Keep the slot open until an acquired:true / error frame arrives. */
  untilGrant: boolean;
}

export class Broker1Client {
  private socket: Socket | null = null;
  private buffer = "";
  private inflight = new Map<string, Waiter>();
  private pid = process.pid;

  constructor(private readonly host: string, private readonly port: number) {}

  async connect(): Promise<void> {
    await new Promise<void>((resolve, reject) => {
      const sock = createConnection({ host: this.host, port: this.port }, () => {
        sock.setNoDelay(true);
        this.socket = sock;
        resolve();
      });
      sock.once("error", reject);
      sock.on("data", (chunk) => this.onData(chunk));
      sock.on("close", () => this.failAll(new Error("connection closed")));
    });
    // Fire-and-forget version handshake.
    this.write({ type: "version", value: "0.2.25" });
  }

  private onData(chunk: Buffer): void {
    this.buffer += chunk.toString("utf8");
    let nl: number;
    while ((nl = this.buffer.indexOf("\n")) >= 0) {
      const line = this.buffer.slice(0, nl);
      this.buffer = this.buffer.slice(nl + 1);
      if (!line.trim()) continue;
      let msg: Record<string, unknown>;
      try {
        msg = JSON.parse(line) as Record<string, unknown>;
      } catch {
        continue;
      }
      const uuid = msg["uuid"] as string | undefined;
      if (!uuid) continue;
      const w = this.inflight.get(uuid);
      if (!w) continue;
      if (w.untilGrant) {
        const acquired = msg["acquired"] === true;
        const hasError = typeof msg["error"] === "string";
        // NOTE: a frame carrying `contendedKey` with acquired:false is the
        // broker's "you're queued" notice, NOT a terminal rejection. While
        // blocking we must keep waiting for the follow-up acquired:true frame.
        if (!acquired && !hasError) continue; // still queued
      }
      this.inflight.delete(uuid);
      w.resolve(msg);
    }
  }

  private failAll(err: Error): void {
    for (const [, w] of this.inflight) w.reject(err);
    this.inflight.clear();
  }

  private write(obj: Record<string, unknown>): void {
    if (!this.socket) throw new Error("not connected");
    this.socket.write(JSON.stringify(obj) + "\n");
  }

  private roundtrip(
    obj: Record<string, unknown>,
    uuid: string,
    untilGrant: boolean,
  ): Promise<Record<string, unknown>> {
    return new Promise((resolve, reject) => {
      this.inflight.set(uuid, { resolve, reject, untilGrant });
      this.write(obj);
    });
  }

  async acquire(key: string, ttlMs = 30_000): Promise<Broker1SingleHandle> {
    const uuid = randomUUID();
    const reply = await this.roundtrip(
      { type: "lock", uuid, key, pid: this.pid, keepLocksAfterDeath: false, ttl: ttlMs > 0 ? ttlMs : null, wait: true },
      uuid,
      true,
    );
    if (reply["acquired"] !== true) {
      throw new Error(`lock(${key}) not acquired: ${reply["error"] ?? JSON.stringify(reply)}`);
    }
    return { key, lockUuid: uuid, fencingToken: Number(reply["fencingToken"] ?? 0) };
  }

  /** Non-blocking single-key acquire. Resolves to null on contention. */
  async tryAcquire(key: string, ttlMs = 30_000): Promise<Broker1SingleHandle | null> {
    const uuid = randomUUID();
    const reply = await this.roundtrip(
      { type: "lock", uuid, key, pid: this.pid, keepLocksAfterDeath: false, ttl: ttlMs > 0 ? ttlMs : null, wait: false },
      uuid,
      false,
    );
    if (typeof reply["error"] === "string") throw new Error(`tryAcquire(${key}) error: ${reply["error"]}`);
    if (reply["acquired"] !== true) return null;
    return { key, lockUuid: uuid, fencingToken: Number(reply["fencingToken"] ?? 0) };
  }

  async release(h: Broker1SingleHandle): Promise<void> {
    const uuid = randomUUID();
    const reply = await this.roundtrip(
      { type: "unlock", uuid, _uuid: h.lockUuid, key: h.key, force: false },
      uuid,
      false,
    );
    if (reply["unlocked"] !== true) {
      throw new Error(`unlock(${h.key}) rejected: ${reply["error"] ?? JSON.stringify(reply)}`);
    }
  }

  async acquireMany(keys: string[], ttlMs = 30_000): Promise<Broker1CompositeHandle> {
    const uuid = randomUUID();
    const reply = await this.roundtrip(
      { type: "acquire-many", uuid, keys, ttl: ttlMs > 0 ? ttlMs : null, wait: true },
      uuid,
      true,
    );
    if (reply["acquired"] !== true) {
      throw new Error(`acquire-many rejected: ${reply["error"] ?? reply["contendedKey"] ?? JSON.stringify(reply)}`);
    }
    return {
      keys: (reply["keys"] as string[]) ?? keys,
      lockUuid: (reply["lockUuid"] as string) ?? "",
      fencingTokens: (reply["fencingTokens"] as Record<string, number>) ?? {},
    };
  }

  /** Non-blocking composite acquire. Resolves to null on contention. */
  async tryAcquireMany(keys: string[], ttlMs = 30_000): Promise<Broker1CompositeHandle | null> {
    const uuid = randomUUID();
    const reply = await this.roundtrip(
      { type: "acquire-many", uuid, keys, ttl: ttlMs > 0 ? ttlMs : null, wait: false },
      uuid,
      false,
    );
    if (typeof reply["error"] === "string") throw new Error(`tryAcquireMany error: ${reply["error"]}`);
    if (reply["acquired"] !== true) return null;
    return {
      keys: (reply["keys"] as string[]) ?? keys,
      lockUuid: (reply["lockUuid"] as string) ?? "",
      fencingTokens: (reply["fencingTokens"] as Record<string, number>) ?? {},
    };
  }

  async releaseMany(h: Broker1CompositeHandle): Promise<void> {
    const uuid = randomUUID();
    const reply = await this.roundtrip({ type: "release-many", uuid, lockUuid: h.lockUuid }, uuid, false);
    if (reply["released"] !== true) {
      throw new Error(`release-many rejected: ${reply["error"] ?? JSON.stringify(reply)}`);
    }
  }

  async close(): Promise<void> {
    this.socket?.destroy();
    this.socket = null;
  }
}
