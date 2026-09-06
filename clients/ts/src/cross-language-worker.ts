import { createInterface } from "node:readline";

import { NetworkMutexClient, type LockHandle } from "./client.ts";

const host = process.env["LIVE_MUTEX_HOST"] ?? "127.0.0.1";
const port = Number(process.env["LIVE_MUTEX_PORT"] ?? "6970");
const lang = process.env["LMX_WORKER_LANG"] ?? "typescript";
const worker = process.env["LMX_WORKER_ID"] ?? `${lang}-0`;
const seed = BigInt(process.env["LMX_WORKER_SEED"] ?? "1");
const ops = Number(process.env["LMX_WORKER_OPS"] ?? "50");
const keyPrefix = process.env["LMX_FUZZ_KEY_PREFIX"] ?? "cross";
const keyCount = Number(process.env["LMX_FUZZ_KEY_COUNT"] ?? "5");

const TTL_MS = 60_000;

function makeRng(seed: bigint) {
  let x = (seed ^ 0x9e37_79b9_7f4a_7c15n) || 1n;
  return {
    below(n: number): number {
      x ^= x >> 12n;
      x ^= x << 25n;
      x ^= x >> 27n;
      x = BigInt.asUintN(64, x);
      return Number((x * 0x2545_f491_4f6c_dd1dn) % BigInt(n));
    },
  };
}

const stdinLines = createInterface({ input: process.stdin });
const ackQueue: string[] = [];
const waiters: Array<(line: string) => void> = [];
stdinLines.on("line", (line) => {
  const waiter = waiters.shift();
  if (waiter) waiter(line);
  else ackQueue.push(line);
});

async function waitAck(): Promise<void> {
  const line = ackQueue.shift() ?? await new Promise<string>((resolve) => waiters.push(resolve));
  if (line.trim() !== "ack") {
    throw new Error(`expected ack from harness, got ${JSON.stringify(line)}`);
  }
}

async function emit(event: unknown): Promise<void> {
  process.stdout.write(`${JSON.stringify(event)}\n`);
  await waitAck();
}

function chooseKeys(rng: ReturnType<typeof makeRng>, keys: string[]): string[] {
  const want = 2 + rng.below(Math.min(3, keys.length - 1));
  const pool = [...keys];
  const out: string[] = [];
  for (let i = 0; i < want && pool.length; i++) {
    out.push(pool.splice(rng.below(pool.length), 1)[0]!);
  }
  return [...new Set(out)].sort();
}

function tokensFor(handle: LockHandle): Record<string, number> {
  if (handle.kind === "single") return { [handle.key]: handle.fencingToken };
  return handle.fencingTokens;
}

async function holdBriefly(rng: ReturnType<typeof makeRng>): Promise<void> {
  const ms = rng.below(4);
  if (ms === 0) await Promise.resolve();
  else await new Promise((resolve) => setTimeout(resolve, ms));
}

async function grantReleaseExclusive(
  client: NetworkMutexClient,
  handle: LockHandle,
  rng: ReturnType<typeof makeRng>,
): Promise<void> {
  await emit({
    event: "grant",
    lang,
    worker,
    lockUuid: handle.lockUuid,
    kind: "exclusive",
    keys: handle.kind === "single" ? [handle.key] : handle.keys,
    tokens: tokensFor(handle),
  });
  await holdBriefly(rng);
  await emit({ event: "release", lang, worker, lockUuid: handle.lockUuid });
  await client.release(handle);
}

async function main(): Promise<void> {
  const rng = makeRng(seed);
  const keys = Array.from({ length: keyCount }, (_, i) => `${keyPrefix}-${i}`);
  const client = new NetworkMutexClient({ host, port, connectTimeoutMs: 5_000 });
  await client.connect();

  try {
    for (let op = 0; op < ops; op++) {
      const roll = rng.below(100);
      if (roll < 30) {
        const key = keys[rng.below(keys.length)]!;
        const h = await client.tryAcquire(key, { ttlMs: TTL_MS });
        if (h) await grantReleaseExclusive(client, h, rng);
      } else if (roll < 50) {
        const key = keys[rng.below(keys.length)]!;
        const h = await client.acquire(key, { ttlMs: TTL_MS, waitMs: TTL_MS });
        await grantReleaseExclusive(client, h, rng);
      } else if (roll < 65) {
        const h = await client.tryAcquireMany(chooseKeys(rng, keys), { ttlMs: TTL_MS });
        if (h) await grantReleaseExclusive(client, h, rng);
      } else if (roll < 75) {
        const h = await client.acquireMany(chooseKeys(rng, keys), { ttlMs: TTL_MS, waitMs: TTL_MS });
        await grantReleaseExclusive(client, h, rng);
      } else if (roll < 92) {
        const key = keys[rng.below(keys.length)]!;
        const h = await client.acquireRead(key);
        await emit({
          event: "grant",
          lang,
          worker,
          lockUuid: h.lockUuid,
          kind: "read",
          keys: [key],
          tokens: { [key]: h.fencingToken },
        });
        await holdBriefly(rng);
        await emit({ event: "release", lang, worker, lockUuid: h.lockUuid });
        await client.releaseRead(key);
      } else {
        const key = keys[rng.below(keys.length)]!;
        const h = await client.acquireWrite(key);
        await emit({
          event: "grant",
          lang,
          worker,
          lockUuid: h.lockUuid,
          kind: "write",
          keys: [key],
          tokens: { [key]: h.fencingToken },
        });
        await holdBriefly(rng);
        await emit({ event: "release", lang, worker, lockUuid: h.lockUuid });
        await client.releaseWrite(key);
      }
    }
  } finally {
    await client.close();
    stdinLines.close();
  }

  process.stdout.write(`${JSON.stringify({ event: "done", lang, worker, ops })}\n`);
}

main().catch((err) => {
  console.error(`[${worker}] ${err instanceof Error ? err.stack ?? err.message : String(err)}`);
  process.exit(1);
});
