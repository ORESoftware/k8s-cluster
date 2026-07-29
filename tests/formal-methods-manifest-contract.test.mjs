import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";
import test from "node:test";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

test("fmctl schema-v1 manifests validate and malformed contracts fail closed", () => {
  const output = execFileSync(
    "python3",
    ["scripts/check-formal-methods-manifests.py", "--self-test"],
    {
      cwd: root,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    },
  );

  for (const repository of [
    "fiducia-node.rs",
    "fiducia-ai-agent-bridge.rs",
    "fiducia-brain.rs",
    "fiducia-ai-agent-control-plane",
  ]) {
    assert.match(output, new RegExp(`validated apps/${repository}:`));
  }
  assert.match(output, /validated fail-closed manifest self-tests/);
});
