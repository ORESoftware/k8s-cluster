// End-to-end smoke test: spin up the local broker (assumed already running on
// :6970) and exercise exclusive, composite, and reader-writer locks.
//
//   pnpm --dir clients/ts smoke
//
// The script exits non-zero on any unexpected response so it can be wired
// into CI.

import { NetworkMutexClient } from "./client.ts";

const HOST = process.env["LIVE_MUTEX_HOST"] ?? "127.0.0.1";
const PORT = Number(process.env["LIVE_MUTEX_PORT"] ?? 6970);

async function main(): Promise<void> {
  const client = new NetworkMutexClient({ host: HOST, port: PORT });
  await client.connect();
  console.log(`[smoke] connected ${HOST}:${PORT}`);

  // Exclusive
  const exHandle = await client.acquire("smoke-ts-exclusive");
  console.log(`[smoke] exclusive grant: lockUuid=${exHandle.lockUuid} fencing=${exHandle.fencingToken}`);
  await client.release(exHandle);
  console.log(`[smoke] released exclusive`);

  // Composite
  const compHandle = await client.acquireMany(["smoke-ts-a", "smoke-ts-b", "smoke-ts-c"]);
  console.log(
    `[smoke] composite grant: lockUuid=${compHandle.lockUuid} tokens=${JSON.stringify(compHandle.fencingTokens)}`,
  );
  await client.release(compHandle);
  console.log(`[smoke] released composite`);

  // Reader-writer: one writer
  const w = await client.acquireWrite("smoke-ts-rw");
  console.log(`[smoke] writer grant: fencing=${w.fencingToken}`);
  await client.releaseWrite("smoke-ts-rw");

  // Multiple readers in parallel
  const a = client.acquireRead("smoke-ts-rw");
  const b = client.acquireRead("smoke-ts-rw");
  const [ra, rb] = await Promise.all([a, b]);
  console.log(`[smoke] reader1 fencing=${ra.fencingToken} reader2 fencing=${rb.fencingToken}`);
  await client.releaseRead("smoke-ts-rw");
  await client.releaseRead("smoke-ts-rw");

  await client.close();
  console.log(`[smoke] OK`);
}

main().catch((err) => {
  console.error("[smoke] FAIL", err);
  process.exitCode = 1;
});
