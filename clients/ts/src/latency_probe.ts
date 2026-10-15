// Latency-focused harness for the upstream `live-mutex#22` socket-tuning
// experiment. Unlike `compare.ts` (which measures aggregate throughput),
// this script does a single sequential acquire/release loop on one TCP
// connection and reports p50 / p95 / p99 / max latency.
//
// The point is to expose the delayed-ACK / Nagle interaction that
// `TCP_QUICKACK` is supposed to fix — that effect only shows up when
// requests are actually waiting on a kernel ACK, i.e. when a connection
// is mostly idle between requests. A high-fanout throughput test hides
// it.
//
// Usage:
//
//   tsx src/latency_probe.ts             # default ITERATIONS=2000, ours=:6970
//   ITERATIONS=5000 tsx src/latency_probe.ts
//   HOST=10.0.0.5 PORT=6970 tsx src/latency_probe.ts
//
// To A/B-test the QUICKACK experiment, run two brokers — one with
// LMX_TCP_QUICKACK=true (default) and one with =false on a different
// port — and probe each. The Prometheus counters
// `dd_rust_network_mutex_tcp_quickack_applied_total` and
// `_tcp_nodelay_applied_total` confirm the option is wired through.

import { performance } from "node:perf_hooks";
import { NetworkMutexClient } from "./client.ts";

const HOST = process.env["HOST"] ?? process.env["LIVE_MUTEX_HOST"] ?? "127.0.0.1";
const PORT = Number(process.env["PORT"] ?? process.env["LIVE_MUTEX_PORT"] ?? 6970);
const ITERATIONS = Number(process.env["ITERATIONS"] ?? 2000);
const KEY = process.env["KEY"] ?? "latency-probe";
const LABEL = process.env["LABEL"] ?? `${HOST}:${PORT}`;
const SLEEP_BETWEEN_MS = Number(process.env["SLEEP_BETWEEN_MS"] ?? 0);

function quantile(sorted: number[], q: number): number {
  if (sorted.length === 0) return 0;
  const pos = (sorted.length - 1) * q;
  const base = Math.floor(pos);
  const rest = pos - base;
  const next = sorted[base + 1];
  const cur = sorted[base]!;
  return next !== undefined ? cur + rest * (next - cur) : cur;
}

function sleepMs(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

async function main(): Promise<void> {
  const client = new NetworkMutexClient({ host: HOST, port: PORT });
  await client.connect();

  // Warmup — first ~50 round-trips might pay the TCP slow-start tax and
  // pollute the percentiles otherwise.
  for (let i = 0; i < Math.min(50, ITERATIONS); i++) {
    const h = await client.acquire(`${KEY}-warmup`, { ttlMs: 5_000, waitMs: 5_000 });
    await client.release(h);
  }

  const samplesNs: bigint[] = [];
  let errors = 0;
  const startedAt = performance.now();
  for (let i = 0; i < ITERATIONS; i++) {
    const t0 = process.hrtime.bigint();
    try {
      const handle = await client.acquire(KEY, { ttlMs: 5_000, waitMs: 5_000 });
      await client.release(handle);
      samplesNs.push(process.hrtime.bigint() - t0);
    } catch {
      errors++;
    }
    if (SLEEP_BETWEEN_MS > 0) await sleepMs(SLEEP_BETWEEN_MS);
  }
  const totalMs = performance.now() - startedAt;
  await client.close();

  const samplesMs = samplesNs.map((n) => Number(n) / 1e6).sort((a, b) => a - b);
  const sum = samplesMs.reduce((a, b) => a + b, 0);
  const avg = sum / Math.max(samplesMs.length, 1);
  const p50 = quantile(samplesMs, 0.5);
  const p95 = quantile(samplesMs, 0.95);
  const p99 = quantile(samplesMs, 0.99);
  const max = samplesMs.length === 0 ? 0 : samplesMs[samplesMs.length - 1]!;

  console.log(`[probe ${LABEL}]`);
  console.log(`  iterations:      ${ITERATIONS}  errors=${errors}`);
  console.log(`  wall time:       ${totalMs.toFixed(1)} ms`);
  console.log(`  throughput:      ${(samplesMs.length / (totalMs / 1000)).toFixed(0)} ops/s`);
  console.log(`  avg latency:     ${avg.toFixed(3)} ms`);
  console.log(`  p50 latency:     ${p50.toFixed(3)} ms`);
  console.log(`  p95 latency:     ${p95.toFixed(3)} ms`);
  console.log(`  p99 latency:     ${p99.toFixed(3)} ms`);
  console.log(`  max latency:     ${max.toFixed(3)} ms`);
}

main().catch((err) => {
  console.error("[probe] FAIL", err);
  process.exitCode = 1;
});
