// Cross-runtime check for caller-controlled wait / no-wait acquisition.
//
// Runs the SAME scenario against both brokers and asserts identical behavior:
//   * no-wait (`tryAcquire*`) returns immediately — null on contention, a
//     handle when free — and never enqueues (so it can't leak a deferred
//     grant: a later acquire of the same key still succeeds promptly);
//   * wait (`acquire*`) blocks on a contended key until the holder releases,
//     then is granted.
//
// Usage:
//   npx tsx src/wait-semantics.ts \
//     OURS_HOST=127.0.0.1 OURS_PORT=6970 THEIRS_HOST=127.0.0.1 THEIRS_PORT=7972

import { NetworkMutexClient } from "./client.ts";
import { Broker1Client } from "./lmx-broker1-client.ts";

const OURS_HOST = process.env["OURS_HOST"] ?? "127.0.0.1";
const OURS_PORT = Number(process.env["OURS_PORT"] ?? 6970);
const THEIRS_HOST = process.env["THEIRS_HOST"] ?? "127.0.0.1";
const THEIRS_PORT = Number(process.env["THEIRS_PORT"] ?? 7972);

const TTL = 30_000;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

interface Handle {
  release(): Promise<void>;
}

/** Uniform façade so the same scenario drives either client. */
interface Session {
  acquire(key: string): Promise<Handle>;
  tryAcquire(key: string): Promise<Handle | null>;
  acquireMany(keys: string[]): Promise<Handle>;
  tryAcquireMany(keys: string[]): Promise<Handle | null>;
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
        return { release: () => c.release(h) };
      },
      async tryAcquire(key) {
        const h = await c.tryAcquire(key, { ttlMs: TTL });
        return h ? { release: () => c.release(h) } : null;
      },
      async acquireMany(keys) {
        const h = await c.acquireMany(keys, { ttlMs: TTL, waitMs: TTL });
        return { release: () => c.release(h) };
      },
      async tryAcquireMany(keys) {
        const h = await c.tryAcquireMany(keys, { ttlMs: TTL });
        return h ? { release: () => c.release(h) } : null;
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
        return { release: () => c.release(h) };
      },
      async tryAcquire(key) {
        const h = await c.tryAcquire(key, TTL);
        return h ? { release: () => c.release(h) } : null;
      },
      async acquireMany(keys) {
        const h = await c.acquireMany(keys, TTL);
        return { release: () => c.releaseMany(h) };
      },
      async tryAcquireMany(keys) {
        const h = await c.tryAcquireMany(keys, TTL);
        return h ? { release: () => c.releaseMany(h) } : null;
      },
      close: () => c.close(),
    };
  },
};

interface Check {
  name: string;
  pass: boolean;
  detail: string;
}

async function runScenario(backend: Backend, tag: string): Promise<Check[]> {
  const checks: Check[] = [];
  const add = (name: string, pass: boolean, detail = "") => checks.push({ name, pass, detail });

  const ns = (s: string) => `${tag}-${s}`;
  const a = await backend.connect();
  const b = await backend.connect();
  try {
    // --- composite no-wait fails fast on contention -----------------------
    const hA = await a.acquireMany([ns("k1"), ns("k2")]);
    const t0 = Date.now();
    const contended = await b.tryAcquireMany([ns("k2"), ns("k3")]);
    const dtContended = Date.now() - t0;
    add("composite no-wait returns null on contention", contended === null, `got ${contended ? "handle" : "null"}`);
    add("composite no-wait returns immediately (<500ms)", dtContended < 500, `${dtContended}ms`);

    // --- composite no-wait does NOT partially lock free members ----------
    // k3 was a free member of the failed attempt; it must still be grabbable.
    const k3only = await b.tryAcquire(ns("k3"));
    add("free member not leaked by failed no-wait composite", k3only !== null, `k3 ${k3only ? "free" : "stuck"}`);
    if (k3only) await k3only.release();

    // --- composite no-wait when fully free succeeds ----------------------
    const freeSet = await b.tryAcquireMany([ns("d1"), ns("d2")]);
    add("composite no-wait succeeds when free", freeSet !== null);
    if (freeSet) await freeSet.release();

    // --- composite WAIT blocks until release, then is granted ------------
    let waitResolved = false;
    const waitPromise = b.acquireMany([ns("k2"), ns("k3")]).then((h) => {
      waitResolved = true;
      return h;
    });
    await sleep(300);
    add("composite wait blocks while contended", waitResolved === false, `resolved=${waitResolved}`);

    await hA.release();
    const granted = await Promise.race([
      waitPromise.then(() => "granted"),
      sleep(5000).then(() => "timeout"),
    ]);
    add("composite wait granted after release", granted === "granted", String(granted));
    if (granted === "granted") {
      const h = await waitPromise;
      await h.release();
    }

    // --- single-key no-wait / wait round-trip ----------------------------
    const sA = await a.acquire(ns("s1"));
    const sContended = await b.tryAcquire(ns("s1"));
    add("single no-wait returns null on contention", sContended === null);
    await sA.release();
    // No leftover waiter: a fresh no-wait acquire now succeeds.
    const sFree = await b.tryAcquire(ns("s1"));
    add("single no-wait succeeds after release (no leaked waiter)", sFree !== null);
    if (sFree) await sFree.release();
  } finally {
    await a.close();
    await b.close();
  }
  return checks;
}

async function main(): Promise<void> {
  let anyFail = false;
  for (const backend of [oursBackend, theirsBackend]) {
    let checks: Check[];
    try {
      checks = await runScenario(backend, backend === oursBackend ? "ours" : "theirs");
    } catch (err) {
      console.error(`\n[wait-semantics] ${backend.name} ERROR: ${(err as Error).message}`);
      anyFail = true;
      continue;
    }
    console.log(`\n[wait-semantics] === ${backend.name} ===`);
    for (const c of checks) {
      const status = c.pass ? "PASS" : "FAIL";
      console.log(`  ${status}  ${c.name}${c.detail ? `   (${c.detail})` : ""}`);
      if (!c.pass) anyFail = true;
    }
  }
  console.log(
    `\n[wait-semantics] VERDICT: ${anyFail ? "FAIL" : "PASS"} — caller-controlled wait/no-wait behaves identically on both brokers.`,
  );
  process.exit(anyFail ? 1 : 0);
}

main().catch((err) => {
  console.error("[wait-semantics] fatal:", err);
  process.exit(1);
});
