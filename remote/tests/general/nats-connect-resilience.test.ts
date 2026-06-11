import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { readdir, readFile } from "node:fs/promises";
import { resolve } from "node:path";
import test from "node:test";

// Guard against the recurring "NATS connect crashes the service at boot"
// regression. `async_nats::connect(url).await?` propagates a connect error out
// of `main`, so a broker that is briefly unreachable at startup turns into a
// CrashLoopBackoff that also takes down the service's HTTP/health surface.
//
// The two correct shapes are:
//   * optional NATS  -> degrade: `match connect(&url).await { Ok(c)=>Some(c), Err(e)=>{ log; None } }`
//   * NATS is core   -> wait:    `ConnectOptions::new().retry_on_initial_connect().connect(url).await?`
//
// A genuinely-intentional crash-on-connect (e.g. a throwaway load-test client)
// can opt out by putting `nats-connect-crash-ok` in a comment on the same line
// or the line immediately above the connect call.

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), "..", "..")]) {
    if (existsSync(resolve(candidate, "remote/argocd/dd-next-runtime/kustomization.yaml"))) {
      return candidate;
    }
  }
  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function rustSourcesUnder(dir: string): Promise<string[]> {
  const out: string[] = [];
  let entries;
  try {
    entries = await readdir(dir, { withFileTypes: true });
  } catch {
    return out;
  }
  for (const entry of entries) {
    if (entry.name === "target" || entry.name === ".git" || entry.name === "node_modules") {
      continue;
    }
    const full = resolve(dir, entry.name);
    if (entry.isDirectory()) {
      out.push(...(await rustSourcesUnder(full)));
    } else if (entry.isFile() && entry.name.endsWith(".rs")) {
      out.push(full);
    }
  }
  return out;
}

// Matches `async_nats::connect(<args>).await?` including multi-line arg lists;
// the trailing `?` (not a `match`/`?`-free await) is what makes it a crash.
const CRASH_PATTERN = /async_nats::connect\([^)]*\)\s*\.await\s*\?/g;

test("no service crashes on a NATS connect failure at boot", async () => {
  const deploymentsRoot = resolve(repoRoot, "remote/deployments");
  const files = await rustSourcesUnder(deploymentsRoot);
  assert.ok(files.length > 0, "expected to find Rust sources under remote/deployments");

  const violations: string[] = [];

  for (const file of files) {
    const source = await readFile(file, "utf8");
    if (!source.includes("async_nats::connect")) {
      continue;
    }
    const lines = source.split("\n");
    for (const match of source.matchAll(CRASH_PATTERN)) {
      const lineNo = source.slice(0, match.index ?? 0).split("\n").length;
      const thisLine = lines[lineNo - 1] ?? "";
      const prevLine = lines[lineNo - 2] ?? "";
      if (
        thisLine.includes("nats-connect-crash-ok") ||
        prevLine.includes("nats-connect-crash-ok")
      ) {
        continue;
      }
      const rel = file.slice(repoRoot.length + 1);
      violations.push(`${rel}:${lineNo}`);
    }
  }

  assert.deepEqual(
    violations,
    [],
    `Found bare \`async_nats::connect(...).await?\` (crashes the service if NATS is ` +
      `down at boot) at:\n  ${violations.join("\n  ")}\n` +
      `Fix: degrade to None for optional NATS, or use ` +
      `ConnectOptions::new().retry_on_initial_connect() when NATS is core. ` +
      `Intentional cases may add a \`nats-connect-crash-ok\` comment.`,
  );
});
