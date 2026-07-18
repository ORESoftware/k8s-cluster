// Regression tests for the audit script's GitHub-Actions pin check
// (scripts/audit-repo-state.sh scan_action_pins). Both writers patched this
// logic to accept docker digest pins; these fixtures make sure a future
// refactor can neither reopen the false positive (digest-pinned docker
// actions flagged) nor introduce a false negative (mutable tags accepted).

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

/** Build a throwaway git repo whose only content is one workflow file. */
function fixtureRepo(workflowYaml) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fiducia-pin-fixture-"));
  fs.mkdirSync(path.join(dir, ".github", "workflows"), { recursive: true });
  fs.writeFileSync(path.join(dir, ".github", "workflows", "ci.yml"), workflowYaml);
  const git = (...args) =>
    execFileSync("git", ["-C", dir, ...args], { stdio: "pipe" });
  git("init", "-q");
  git("add", "-A");
  git("-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "fixture");
  return dir;
}

/** Run scan_action_pins from the real audit script against a fixture repo. */
function scan(repoDir) {
  const script = [
    "set -u",
    "failures=0",
    'fail() { failures=$((failures + 1)); echo "FAIL: $*" >&2; }',
    // Extract the function verbatim from the audited script.
    `source /dev/stdin <<'FN'\n${extractFunction()}\nFN`,
    `scan_action_pins ${JSON.stringify(repoDir)} fixture`,
    'echo "failures=$failures"',
  ].join("\n");
  const out = execFileSync("bash", ["-c", script], { stdio: "pipe" }).toString();
  return Number(out.match(/failures=(\d+)/)[1]);
}

function extractFunction() {
  const source = fs.readFileSync(
    path.join(root, "scripts", "audit-repo-state.sh"),
    "utf8",
  );
  const match = source.match(/scan_action_pins\(\) \{[\s\S]*?\n\}/);
  assert.ok(match, "scan_action_pins not found in audit script");
  return match[0];
}

test("immutable pins pass: 40-hex commit SHAs and docker sha256 digests", () => {
  const repo = fixtureRepo(
    [
      "jobs:",
      "  ci:",
      "    steps:",
      "      - uses: actions/checkout@8f4b7f84864484a7bf31766abe9204da3cbe65b3",
      "      - uses: docker://rhysd/actionlint@sha256:" + "b".repeat(64),
      "",
    ].join("\n"),
  );
  assert.equal(scan(repo), 0, "immutable pins must not be flagged");
});

test("mutable references fail: version tags, branches, and undigested docker", () => {
  for (const uses of [
    "actions/checkout@v4",
    "actions/setup-node@main",
    "docker://rhysd/actionlint:1.7.4",
  ]) {
    const repo = fixtureRepo(`jobs:\n  ci:\n    steps:\n      - uses: ${uses}\n`);
    assert.equal(scan(repo), 1, `${uses} must be flagged as mutable`);
  }
});
