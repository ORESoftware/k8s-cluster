// Apples-to-apples throughput comparison between:
//
//   ours: rust-network-mutex-rs broker (this deployment)
//   them: oresoftware/live-mutex broker (upstream npm package)
//
// Both brokers are spoken to from the same Node process, so the workload
// (`WORKERS` concurrent acquire/release loops on `KEYS` random keys for
// `DURATION_MS` ms) is identical and the reported numbers are directly
// comparable.
//
// Defaults are intentionally small so this can run in CI; bump via env.
//
//   WORKERS=64 KEYS=8 DURATION_MS=5000 \
//     OURS_HOST=127.0.0.1 OURS_PORT=6970 \
//     THEIRS_HOST=127.0.0.1 THEIRS_PORT=6971 \
//     tsx src/compare.ts

import { performance } from "node:perf_hooks";
import { NetworkMutexClient } from "./client.ts";

// Upstream live-mutex types are CJS; we import dynamically. The relevant
// promise-flavoured surface is `acquire(key, opts)` / `release(key, { id })`,
// which is what `live-mutex-loadtest-node` uses too.
interface LiveMutexAcquireResult {
  key?: string;
  id?: string;
  lockUuid?: string;
}
interface LiveMutexClient {
  ensure(): Promise<unknown>;
  acquire(
    key: string,
    opts?: { ttl?: number; lockRequestTimeout?: number; maxRetries?: number },
  ): Promise<LiveMutexAcquireResult>;
  release(
    key: string,
    opts?: { id?: string; unlockRequestTimeout?: number },
  ): Promise<unknown>;
  close?(): Promise<void>;
}

const WORKERS = Number(process.env["WORKERS"] ?? 16);
const KEYS = Number(process.env["KEYS"] ?? 4);
const DURATION_MS = Number(process.env["DURATION_MS"] ?? 2000);

const OURS_HOST = process.env["OURS_HOST"] ?? "127.0.0.1";
const OURS_PORT = Number(process.env["OURS_PORT"] ?? 6970);
const THEIRS_HOST = process.env["THEIRS_HOST"] ?? "127.0.0.1";
const THEIRS_PORT = Number(process.env["THEIRS_PORT"] ?? 6971);

interface Stats {
  total: number;
  errors: number;
  totalLatencyNs: bigint;
  maxLatencyNs: bigint;
}

function newStats(): Stats {
  return { total: 0, errors: 0, totalLatencyNs: 0n, maxLatencyNs: 0n };
}

function pickKey(): string {
  return `compare-${Math.floor(Math.random() * KEYS)}`;
}

async function runOurs(stats: Stats, deadline: number): Promise<void> {
  const client = new NetworkMutexClient({ host: OURS_HOST, port: OURS_PORT });
  await client.connect();
  try {
    while (performance.now() < deadline) {
      const key = pickKey();
      const t0 = process.hrtime.bigint();
      try {
        const handle = await client.acquire(key, { ttlMs: 5_000, waitMs: 5_000 });
        await client.release(handle);
        const dt = process.hrtime.bigint() - t0;
        stats.total++;
        stats.totalLatencyNs += dt;
        if (dt > stats.maxLatencyNs) stats.maxLatencyNs = dt;
      } catch {
        stats.errors++;
      }
    }
  } finally {
    await client.close();
  }
}

async function runTheirs(stats: Stats, deadline: number): Promise<void> {
  let lm: LiveMutexClient;
  try {
    const mod = (await import("live-mutex")) as unknown as {
      Client: new (opts: { host: string; port: number; ttl?: number; noDelay?: boolean }) => LiveMutexClient;
    };
    lm = new mod.Client({ host: THEIRS_HOST, port: THEIRS_PORT, ttl: 5_000, noDelay: true });
    await lm.ensure();
  } catch (err) {
    stats.errors++;
    console.error("[compare] live-mutex not available, skipping THEIRS:", (err as Error).message);
    return;
  }
  try {
    while (performance.now() < deadline) {
      const key = pickKey();
      const t0 = process.hrtime.bigint();
      try {
        const grant = await lm.acquire(key, { ttl: 5_000, lockRequestTimeout: 5_000 });
        const id = grant.id ?? grant.lockUuid;
        await lm.release(grant.key ?? key, { id, unlockRequestTimeout: 5_000 });
        const dt = process.hrtime.bigint() - t0;
        stats.total++;
        stats.totalLatencyNs += dt;
        if (dt > stats.maxLatencyNs) stats.maxLatencyNs = dt;
      } catch {
        stats.errors++;
      }
    }
  } finally {
    await lm.close?.();
  }
}

function reportRow(label: string, stats: Stats, durationMs: number): void {
  const seconds = durationMs / 1000;
  const throughput = stats.total / seconds;
  const avgLatencyMs = stats.total === 0 ? 0 : Number(stats.totalLatencyNs / BigInt(stats.total)) / 1e6;
  const maxLatencyMs = Number(stats.maxLatencyNs) / 1e6;
  console.log(
    `${label.padEnd(8)}  total=${stats.total.toString().padStart(7)}  ` +
      `throughput=${throughput.toFixed(0).padStart(8)} ops/s  ` +
      `avg=${avgLatencyMs.toFixed(2).padStart(7)}ms  ` +
      `max=${maxLatencyMs.toFixed(2).padStart(7)}ms  ` +
      `errors=${stats.errors}`,
  );
}

async function compare(label: "ours" | "theirs"): Promise<Stats> {
  const stats = newStats();
  const deadline = performance.now() + DURATION_MS;
  const tasks = Array.from({ length: WORKERS }, () =>
    label === "ours" ? runOurs(stats, deadline) : runTheirs(stats, deadline),
  );
  await Promise.all(tasks);
  return stats;
}

async function main(): Promise<void> {
  console.log(
    `[compare] workers=${WORKERS} keys=${KEYS} duration=${DURATION_MS}ms ` +
      `ours=${OURS_HOST}:${OURS_PORT} theirs=${THEIRS_HOST}:${THEIRS_PORT}`,
  );

  const ours = await compare("ours");
  reportRow("ours", ours, DURATION_MS);

  const theirs = await compare("theirs");
  reportRow("theirs", theirs, DURATION_MS);

  if (ours.total > 0 && theirs.total > 0) {
    const ratio = ours.total / theirs.total;
    console.log(`[compare] ratio (ours / theirs) = ${ratio.toFixed(2)}x`);
  }
}

main().catch((err) => {
  console.error("[compare] FAIL", err);
  process.exitCode = 1;
});
