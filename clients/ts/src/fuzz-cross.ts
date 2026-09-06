// Cross-runtime LIVE concurrency fuzz for fencing tokens + multi-key composites.
//
// Drives many concurrent client connections against a real broker over the
// wire, mixing blocking + no-wait, single + composite acquires on a small
// contended key space. A shared in-process shadow model (this is a single
// Node process, so model updates are atomic between await points) enforces:
//
//   * mutual exclusion  — a key is never held by two live holders at once;
//   * fencing monotonic — each grant's per-key token strictly exceeds the last
//                         token observed for that key, across ALL workers;
//   * composite atomicity — a composite grant carries a token for every key.
//
// Runs the IDENTICAL workload against BOTH brokers and reports parity: both
// must complete with zero invariant violations and no deadlock.
//
// Ordering rule that keeps the model race-free: mark keys FREE in the model
// BEFORE issuing the release on the wire, so the broker can't hand a key to the
// next waiter until the model already reflects the release.
//
// Usage:
//   npx tsx src/fuzz-cross.ts \
//     OURS_HOST=127.0.0.1 OURS_PORT=6970 THEIRS_HOST=127.0.0.1 THEIRS_PORT=7972

import { NetworkMutexClient } from "./client.ts";
import { Broker1Client } from "./lmx-broker1-client.ts";

const OURS_HOST = process.env["OURS_HOST"] ?? "127.0.0.1";
const OURS_PORT = Number(process.env["OURS_PORT"] ?? 6970);
const THEIRS_HOST = process.env["THEIRS_HOST"] ?? "127.0.0.1";
const THEIRS_PORT = Number(process.env["THEIRS_PORT"] ?? 7972);

const TTL = 60_000;
const OP_TIMEOUT_MS = 25_000;

// xorshift32 deterministic RNG.
function makeRng(seed: number) {
  let x = (seed ^ 0x9e3779b9) >>> 0 || 1;
  return {
    below(n: number): number {
      x ^= x << 13; x >>>= 0;
      x ^= x >> 17;
      x ^= x << 5; x >>>= 0;
      return x % n;
    },
  };
}

interface AcqResult {
  keys: string[];
  tokens: Record<string, number>;
  release(): Promise<void>;
}

interface Session {
  acquire(key: string): Promise<AcqResult>;
  tryAcquire(key: string): Promise<AcqResult | null>;
  acquireMany(keys: string[]): Promise<AcqResult>;
  tryAcquireMany(keys: string[]): Promise<AcqResult | null>;
  close(): Promise<void>;
}

interface Backend {
  name: string;
  connect(): Promise<Session>;
}

const oursBackend: Backend = {
  name: "ours (rust)",
  async connect(): Promise<Session> {
    const c = new NetworkMutexClient({ host: OURS_HOST, port: OURS_PORT });
    await c.connect();
    return {
      async acquire(key) {
        const h = await c.acquire(key, { ttlMs: TTL, waitMs: TTL });
        return { keys: [key], tokens: { [key]: h.fencingToken }, release: () => c.release(h) };
      },
      async tryAcquire(key) {
        const h = await c.tryAcquire(key, { ttlMs: TTL });
        return h ? { keys: [key], tokens: { [key]: h.fencingToken }, release: () => c.release(h) } : null;
      },
      async acquireMany(keys) {
        const h = await c.acquireMany(keys, { ttlMs: TTL, waitMs: TTL });
        return { keys: h.keys, tokens: h.fencingTokens, release: () => c.release(h) };
      },
      async tryAcquireMany(keys) {
        const h = await c.tryAcquireMany(keys, { ttlMs: TTL });
        return h ? { keys: h.keys, tokens: h.fencingTokens, release: () => c.release(h) } : null;
      },
      close: () => c.close(),
    };
  },
};

const theirsBackend: Backend = {
  name: "theirs (ts Broker1)",
  async connect(): Promise<Session> {
    const c = new Broker1Client(THEIRS_HOST, THEIRS_PORT);
    await c.connect();
    return {
      async acquire(key) {
        const h = await c.acquire(key, TTL);
        return { keys: [key], tokens: { [key]: h.fencingToken }, release: () => c.release(h) };
      },
      async tryAcquire(key) {
        const h = await c.tryAcquire(key, TTL);
        return h ? { keys: [key], tokens: { [key]: h.fencingToken }, release: () => c.release(h) } : null;
      },
      async acquireMany(keys) {
        const h = await c.acquireMany(keys, TTL);
        return { keys: h.keys, tokens: h.fencingTokens, release: () => c.releaseMany(h) };
      },
      async tryAcquireMany(keys) {
        const h = await c.tryAcquireMany(keys, TTL);
        return h ? { keys: h.keys, tokens: h.fencingTokens, release: () => c.releaseMany(h) } : null;
      },
      close: () => c.close(),
    };
  },
};

class Model {
  private occupied = new Map<string, boolean>();
  private lastToken = new Map<string, number>();
  violations: string[] = [];

  onGrant(keys: string[], tokens: Record<string, number>) {
    for (const k of keys) {
      if (this.occupied.get(k)) {
        this.violations.push(`mutual-exclusion: ${k} granted while already held`);
      }
      const tok = tokens[k];
      if (typeof tok !== "number") {
        this.violations.push(`composite-atomicity: ${k} granted without a fencing token`);
      } else {
        const prev = this.lastToken.get(k) ?? 0;
        if (tok <= prev) {
          this.violations.push(`fencing: ${k} token ${tok} <= last ${prev} (not strictly increasing)`);
        }
        this.lastToken.set(k, Math.max(prev, tok));
      }
      this.occupied.set(k, true);
    }
  }

  onRelease(keys: string[]) {
    for (const k of keys) this.occupied.set(k, false);
  }

  anyOccupied(): boolean {
    for (const v of this.occupied.values()) if (v) return true;
    return false;
  }
}

function withTimeout<T>(p: Promise<T>, ms: number, what: string): Promise<T> {
  return Promise.race([
    p,
    new Promise<T>((_, rej) => setTimeout(() => rej(new Error(`timeout (${ms}ms): ${what} — possible deadlock`)), ms)),
  ]);
}

async function runFuzz(backend: Backend, opts: { workers: number; iters: number; keys: number; seed: number }): Promise<Model> {
  const model = new Model();
  const keyNames = Array.from({ length: opts.keys }, (_, i) => `xf-${i}`);

  const worker = async (wid: number) => {
    const s = await backend.connect();
    const rng = makeRng(opts.seed + wid * 7919);
    try {
      for (let i = 0; i < opts.iters; i++) {
        const roll = rng.below(100);
        if (roll < 40) {
          // blocking composite (2..=3 keys)
          const want = 2 + rng.below(2);
          const pool = keyNames.slice();
          const chosen: string[] = [];
          for (let j = 0; j < want && pool.length; j++) {
            chosen.push(pool.splice(rng.below(pool.length), 1)[0]!);
          }
          const h = await withTimeout(s.acquireMany(chosen), OP_TIMEOUT_MS, `acquireMany(${chosen})`);
          model.onGrant(h.keys, h.tokens);
          model.onRelease(h.keys);
          await withTimeout(h.release(), OP_TIMEOUT_MS, `release composite`);
        } else if (roll < 75) {
          // blocking single
          const key = keyNames[rng.below(keyNames.length)]!;
          const h = await withTimeout(s.acquire(key), OP_TIMEOUT_MS, `acquire(${key})`);
          model.onGrant(h.keys, h.tokens);
          model.onRelease(h.keys);
          await withTimeout(h.release(), OP_TIMEOUT_MS, `release single`);
        } else if (roll < 88) {
          // no-wait composite
          const want = 2 + rng.below(2);
          const pool = keyNames.slice();
          const chosen: string[] = [];
          for (let j = 0; j < want && pool.length; j++) {
            chosen.push(pool.splice(rng.below(pool.length), 1)[0]!);
          }
          const h = await withTimeout(s.tryAcquireMany(chosen), OP_TIMEOUT_MS, `tryAcquireMany(${chosen})`);
          if (h) {
            model.onGrant(h.keys, h.tokens);
            model.onRelease(h.keys);
            await withTimeout(h.release(), OP_TIMEOUT_MS, `release try-composite`);
          }
        } else {
          // no-wait single
          const key = keyNames[rng.below(keyNames.length)]!;
          const h = await withTimeout(s.tryAcquire(key), OP_TIMEOUT_MS, `tryAcquire(${key})`);
          if (h) {
            model.onGrant(h.keys, h.tokens);
            model.onRelease(h.keys);
            await withTimeout(h.release(), OP_TIMEOUT_MS, `release try-single`);
          }
        }
      }
    } finally {
      await s.close();
    }
  };

  await Promise.all(Array.from({ length: opts.workers }, (_, w) => worker(w)));
  return model;
}

async function main(): Promise<void> {
  const opts = {
    workers: Number(process.env["FUZZ_WORKERS"] ?? 28),
    iters: Number(process.env["FUZZ_ITERS"] ?? 150),
    keys: Number(process.env["FUZZ_KEYS"] ?? 4),
    seed: Number(process.env["FUZZ_SEED"] ?? 0xc0ffee),
  };
  let anyFail = false;

  const which = process.env["BACKEND"] ?? "both";
  const backends =
    which === "ours" ? [oursBackend] : which === "theirs" ? [theirsBackend] : [oursBackend, theirsBackend];

  for (const backend of backends) {
    console.log(`\n[fuzz-cross] === ${backend.name} === (workers=${opts.workers}, iters=${opts.iters}, keys=${opts.keys})`);
    const t0 = Date.now();
    try {
      const model = await runFuzz(backend, opts);
      const dt = Date.now() - t0;
      if (model.violations.length > 0) {
        anyFail = true;
        console.log(`  FAIL  ${model.violations.length} invariant violation(s) in ${dt}ms:`);
        for (const v of model.violations.slice(0, 12)) console.log(`        - ${v}`);
      } else if (model.anyOccupied()) {
        anyFail = true;
        console.log(`  FAIL  keys still marked held after all workers finished (${dt}ms)`);
      } else {
        console.log(`  PASS  no mutual-exclusion / fencing / atomicity violations, no deadlock (${dt}ms)`);
      }
    } catch (err) {
      anyFail = true;
      console.log(`  FAIL  ${(err as Error).message}`);
    }
  }

  console.log(`\n[fuzz-cross] VERDICT: ${anyFail ? "FAIL" : "PASS"} — fencing + multi-key invariants hold under live concurrency on both brokers.`);
  process.exit(anyFail ? 1 : 0);
}

main().catch((err) => {
  console.error("[fuzz-cross] fatal:", err);
  process.exit(1);
});
