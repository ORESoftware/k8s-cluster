// TCP client for the rust-network-mutex-rs broker. The transport is
// newline-delimited JSON; one TCP connection multiplexes many in-flight
// requests, correlated by `uuid`.
//
// We deliberately model `Request` / `Response` as discriminated unions so the
// switch statement below is exhaustiveness-checked by `tsc`. This is the
// TypeScript analogue of the Rust serde-tagged enum and the property the
// upstream `live-mutex` library lacks.

import { createConnection, type Socket } from "node:net";
import { randomUUID } from "node:crypto";
import {
  assertNever,
  type LockRequest,
  type Request,
  type Response,
  type UnlockRequest,
} from "./protocol.ts";

export interface ClientOptions {
  host?: string;
  port?: number;
  /** Optional shared secret. If set, an `auth` request is sent on connect. */
  token?: string;
  /** Total dial timeout, ms. */
  connectTimeoutMs?: number;
}

export interface AcquireOptions {
  /** TTL hint for the broker (ms). 0 means "no TTL". */
  ttlMs?: number;
  /** Per-request acquire deadline (ms). */
  waitMs?: number;
}

export interface TryAcquireOptions {
  /** TTL hint for the broker (ms). 0 means "no TTL". */
  ttlMs?: number;
  /** Deadline to receive the broker's (immediate) reply (ms). */
  replyTimeoutMs?: number;
}

export interface SingleLockHandle {
  kind: "single";
  key: string;
  lockUuid: string;
  fencingToken: number;
}

export interface CompositeLockHandle {
  kind: "composite";
  keys: string[];
  lockUuid: string;
  fencingTokens: Record<string, number>;
}

export type LockHandle = SingleLockHandle | CompositeLockHandle;

interface Inflight {
  resolve(resp: Response): void;
  reject(err: Error): void;
  /** Whether to keep this slot open after the first response (used for the
   * `lock` request, which can produce a `queued`-style update before the
   * actual grant). */
  multi: boolean;
}

export class NetworkMutexClient {
  private socket: Socket | null = null;
  private buffer = "";
  private inflight = new Map<string, Inflight>();
  private connected = false;

  constructor(private readonly opts: ClientOptions = {}) {}

  async connect(): Promise<void> {
    if (this.connected) return;
    const host = this.opts.host ?? "127.0.0.1";
    const port = this.opts.port ?? 6970;
    const timeoutMs = this.opts.connectTimeoutMs ?? 5_000;

    await new Promise<void>((resolve, reject) => {
      const sock = createConnection({ host, port }, () => {
        sock.setNoDelay(true);
        this.socket = sock;
        this.connected = true;
        resolve();
      });
      const t = setTimeout(() => {
        sock.destroy(new Error(`connect timeout after ${timeoutMs}ms`));
      }, timeoutMs);
      sock.once("error", (err) => {
        clearTimeout(t);
        reject(err);
      });
      sock.on("data", (chunk) => this.onData(chunk));
      sock.on("close", () => {
        this.connected = false;
        const err = new Error("connection closed");
        for (const inf of this.inflight.values()) inf.reject(err);
        this.inflight.clear();
      });
    });

    if (this.opts.token) {
      const resp = await this.send(
        { type: "auth", uuid: randomUUID(), token: this.opts.token },
        { multi: false },
      );
      if (resp.type !== "auth" || !resp.ok) {
        throw new Error(`auth failed: ${JSON.stringify(resp)}`);
      }
    }
  }

  async close(): Promise<void> {
    this.connected = false;
    this.socket?.end();
    this.socket = null;
  }

  /** Send a typed request and await the next correlated response. */
  send(req: Request, { multi = false }: { multi?: boolean } = {}): Promise<Response> {
    if (!this.socket || !this.connected) {
      return Promise.reject(new Error("not connected"));
    }
    const uuid = req.uuid;
    return new Promise<Response>((resolve, reject) => {
      this.inflight.set(uuid, { resolve, reject, multi });
      this.socket!.write(JSON.stringify(req) + "\n", (err) => {
        if (err) {
          this.inflight.delete(uuid);
          reject(err);
        }
      });
    });
  }

  /** Acquire a single-key exclusive lock, blocking until granted. */
  async acquire(key: string, opts: AcquireOptions = {}): Promise<SingleLockHandle> {
    const req: LockRequest = {
      type: "lock",
      uuid: randomUUID(),
      key,
      ttl: opts.ttlMs ?? 30_000,
      keepLocksAfterDeath: false,
      wait: true,
    };
    const grant = await this.awaitGrant(req, opts.waitMs ?? 30_000);
    if (grant.type !== "lock" || !grant.acquired || !grant.lockUuid) {
      throw new Error(`acquire(${key}) failed: ${JSON.stringify(grant)}`);
    }
    return {
      kind: "single",
      key,
      lockUuid: grant.lockUuid,
      fencingToken: grant.fencingToken ?? 0,
    };
  }

  /** Acquire a composite (multi-key) exclusive lock atomically, blocking
   * until every key is granted. */
  async acquireMany(keys: string[], opts: AcquireOptions = {}): Promise<CompositeLockHandle> {
    if (keys.length === 0 || keys.length > 5) {
      throw new Error(`composite key count must be 1..=5, got ${keys.length}`);
    }
    const req: LockRequest = {
      type: "lock",
      uuid: randomUUID(),
      keys,
      ttl: opts.ttlMs ?? 30_000,
      keepLocksAfterDeath: false,
      wait: true,
    };
    const grant = await this.awaitGrant(req, opts.waitMs ?? 30_000);
    if (grant.type !== "compositeLock" || !grant.acquired || !grant.lockUuid) {
      throw new Error(`acquireMany([${keys.join(",")}]) failed: ${JSON.stringify(grant)}`);
    }
    return {
      kind: "composite",
      keys,
      lockUuid: grant.lockUuid,
      fencingTokens: grant.fencingTokens ?? {},
    };
  }

  /** Non-blocking single-key acquire. Resolves to `null` immediately if the
   * key is contended (the broker does not enqueue the request). */
  async tryAcquire(key: string, opts: TryAcquireOptions = {}): Promise<SingleLockHandle | null> {
    const req: LockRequest = {
      type: "lock",
      uuid: randomUUID(),
      key,
      ttl: opts.ttlMs ?? 30_000,
      keepLocksAfterDeath: false,
      wait: false,
    };
    const resp = await this.send(req, { multi: false });
    if (resp.type === "error") throw new Error(`tryAcquire(${key}) error: ${resp.error}`);
    if (resp.type !== "lock") throw new Error(`tryAcquire(${key}) unexpected: ${resp.type}`);
    if (!resp.acquired || !resp.lockUuid) return null;
    return { kind: "single", key, lockUuid: resp.lockUuid, fencingToken: resp.fencingToken ?? 0 };
  }

  /** Non-blocking composite acquire. Resolves to `null` immediately if any
   * member key is contended; otherwise grabs all keys atomically. */
  async tryAcquireMany(
    keys: string[],
    opts: TryAcquireOptions = {},
  ): Promise<CompositeLockHandle | null> {
    if (keys.length === 0 || keys.length > 5) {
      throw new Error(`composite key count must be 1..=5, got ${keys.length}`);
    }
    const req: LockRequest = {
      type: "lock",
      uuid: randomUUID(),
      keys,
      ttl: opts.ttlMs ?? 30_000,
      keepLocksAfterDeath: false,
      wait: false,
    };
    const resp = await this.send(req, { multi: false });
    if (resp.type === "error") throw new Error(`tryAcquireMany error: ${resp.error}`);
    if (resp.type !== "compositeLock") throw new Error(`tryAcquireMany unexpected: ${resp.type}`);
    if (!resp.acquired || !resp.lockUuid) return null;
    return { kind: "composite", keys, lockUuid: resp.lockUuid, fencingTokens: resp.fencingTokens ?? {} };
  }

  /** Release a previously held lock (single or composite). */
  async release(handle: LockHandle): Promise<void> {
    const req: UnlockRequest = handle.kind === "single"
      ? { type: "unlock", uuid: randomUUID(), key: handle.key, lockUuid: handle.lockUuid }
      : { type: "unlock", uuid: randomUUID(), keys: handle.keys, lockUuid: handle.lockUuid };
    const resp = await this.send(req, { multi: false });
    if (resp.type !== "unlock" || !resp.unlocked) {
      throw new Error(`release failed: ${JSON.stringify(resp)}`);
    }
  }

  /** Acquire a reader hold. Multiple readers can hold simultaneously. */
  async acquireRead(key: string): Promise<{ lockUuid: string; fencingToken: number }> {
    const req: Request = { type: "registerRead", uuid: randomUUID(), key };
    const resp = await this.awaitRwGrant(req, "registerReadResult");
    return { lockUuid: resp.lockUuid ?? "", fencingToken: resp.fencingToken ?? 0 };
  }

  async releaseRead(key: string): Promise<void> {
    await this.send({ type: "endRead", uuid: randomUUID(), key }, { multi: false });
  }

  async acquireWrite(key: string): Promise<{ lockUuid: string; fencingToken: number }> {
    const req: Request = { type: "registerWrite", uuid: randomUUID(), key };
    const resp = await this.awaitRwGrant(req, "registerWriteResult");
    return { lockUuid: resp.lockUuid ?? "", fencingToken: resp.fencingToken ?? 0 };
  }

  async releaseWrite(key: string): Promise<void> {
    await this.send({ type: "endWrite", uuid: randomUUID(), key }, { multi: false });
  }

  // -- internals -----------------------------------------------------------

  private onData(chunk: Buffer): void {
    this.buffer += chunk.toString("utf8");
    let nl: number;
    while ((nl = this.buffer.indexOf("\n")) >= 0) {
      const line = this.buffer.slice(0, nl).trim();
      this.buffer = this.buffer.slice(nl + 1);
      if (!line) continue;
      let resp: Response;
      try {
        resp = JSON.parse(line) as Response;
      } catch (err) {
        // Bad frame; surface to whoever owns the next inflight request, if any.
        const next = this.inflight.values().next().value;
        if (next) next.reject(new Error(`bad frame: ${(err as Error).message}`));
        continue;
      }
      this.dispatch(resp);
    }
  }

  /** Route an incoming response to its inflight handler. The switch is
   * exhaustiveness-checked at compile time by `assertNever`. */
  private dispatch(resp: Response): void {
    const uuid = resp.uuid;
    const inf = this.inflight.get(uuid);
    if (!inf) {
      // No handler — this can legitimately happen for `reelection` events
      // sent without a correlated request. We swallow them rather than
      // throwing, which matches the Rust client's behaviour.
      return;
    }

    switch (resp.type) {
      case "version":
      case "auth":
      case "unlock":
      case "endReadResult":
      case "endWriteResult":
      case "lockInfo":
      case "lsResult":
      case "ok":
      case "error":
        this.inflight.delete(uuid);
        inf.resolve(resp);
        return;
      case "lock":
      case "compositeLock":
        // A blocking (multi) acquire may receive an interim acquired:false
        // "queued" notice before the real grant — keep the slot open for it.
        // A non-blocking (try) acquire resolves on the first reply.
        if (resp.acquired || resp.error) {
          this.inflight.delete(uuid);
          inf.resolve(resp);
        } else if (!inf.multi) {
          this.inflight.delete(uuid);
          inf.resolve(resp);
        }
        // else: keep slot open for the eventual grant
        return;
      case "registerReadResult":
      case "registerWriteResult":
        if (resp.granted) {
          this.inflight.delete(uuid);
          inf.resolve(resp);
        }
        return;
      case "reelection":
        return;
      default:
        return assertNever(resp);
    }
  }

  private async awaitGrant(req: Request, waitMs: number): Promise<Response> {
    const timeoutHandle = (() => {
      let id: NodeJS.Timeout | undefined;
      const promise = new Promise<Response>((_, reject) => {
        id = setTimeout(() => {
          this.inflight.delete(req.uuid);
          reject(new Error(`grant timeout after ${waitMs}ms`));
        }, waitMs);
      });
      return { promise, cancel: () => id !== undefined && clearTimeout(id) };
    })();
    try {
      const sendPromise = this.send(req, { multi: true });
      const winner = await Promise.race([sendPromise, timeoutHandle.promise]);
      return winner;
    } finally {
      timeoutHandle.cancel();
    }
  }

  private async awaitRwGrant(
    req: Request,
    expected: "registerReadResult" | "registerWriteResult",
  ): Promise<RegisterReadOrWriteResponse> {
    const resp = await this.send(req, { multi: true });
    if (resp.type !== expected) {
      throw new Error(`expected ${expected}, got ${resp.type}`);
    }
    return resp as RegisterReadOrWriteResponse;
  }
}

type RegisterReadOrWriteResponse = Extract<
  Response,
  { type: "registerReadResult" } | { type: "registerWriteResult" }
>;
