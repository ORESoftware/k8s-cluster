// Guard tests for diff.mjs that need no database connection. They drive the
// script as a subprocess with --parse-only so no catalog query is opened.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const diffScript = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "diff.mjs");

function runDiff(extraArgs) {
  return spawnSync(process.execPath, [diffScript, "--parse-only", ...extraArgs], {
    encoding: "utf8",
  });
}

test("valid --env values are accepted", () => {
  for (const env of ["dev", "prod", "staging-2", "us_east.1"]) {
    const result = runDiff([`--env=${env}`]);
    assert.equal(result.status, 0, `--env=${env} should be accepted; stderr: ${result.stderr}`);
  }
});

test("path-traversal --env is rejected before any filesystem or DB access", () => {
  for (const env of ["../../etc/passwd", "..", "a/b", "a\\b", ".env", "-x", ""]) {
    const result = runDiff([`--env=${env}`]);
    assert.equal(result.status, 1, `--env=${JSON.stringify(env)} should exit 1`);
    assert.match(result.stderr, /not a valid environment name/);
  }
});
