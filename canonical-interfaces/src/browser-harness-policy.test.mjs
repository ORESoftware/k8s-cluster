import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const runner = readFileSync(
  new URL("../tests/interface-wasm-browser/run.sh", import.meta.url),
  "utf8",
);
const driver = readFileSync(
  new URL("../tests/interface-wasm-browser/driver.mjs", import.meta.url),
  "utf8",
);
const contract = readFileSync(
  new URL("../tests/interface-wasm-browser/contract.mjs", import.meta.url),
  "utf8",
);
const workflow = readFileSync(
  new URL("../.github/workflows/ci.yml", import.meta.url),
  "utf8",
);

test("Chromium is supervised by bounded wall clock rather than virtual time", () => {
  for (const token of [
    "CANONICAL_INTERFACE_BROWSER_TIMEOUT_SECONDS",
    "command -v \"$command\"",
    'timeout --signal=TERM --kill-after=5s "${wall_timeout}s"',
    '"$driver_status" -eq 124',
    "wall-clock limit",
    "--remote-debugging-address=127.0.0.1",
    "--host-resolver-rules=\"MAP * ~NOTFOUND, EXCLUDE 127.0.0.1\"",
    'setsid "$chrome_bin"',
    'kill -TERM -- "-$chrome_pid"',
    'kill -KILL -- "-$chrome_pid"',
    "local status=$?",
    'exit "$status"',
  ]) {
    assert.ok(runner.includes(token), `browser runner must retain ${token}`);
  }

  assert.ok(!runner.includes("--virtual-time-budget"));
  assert.ok(!runner.includes("--dump-dom"));
  assert.ok(!contract.includes("Promise.race"));
  assert.ok(!contract.includes("setTimeout"));
  assert.ok(!contract.includes("withTimeout"));
});

test("DevTools waits for the explicit asynchronous contract sentinel", () => {
  for (const token of [
    "MAX_DISCOVERY_BYTES",
    'redirect: "error"',
    "assertLoopbackHttp",
    "Target.createTarget",
    "Target.attachToTarget",
    "Page.navigate",
    "Runtime.evaluate",
    "MutationObserver",
    "awaitPromise: true",
    "document.documentElement.outerHTML",
    'result.status !== "pass"',
  ]) {
    assert.ok(driver.includes(token), `DevTools driver must retain ${token}`);
  }

  assert.ok(!driver.includes("Promise.race"));
  assert.ok(!driver.includes("setTimeout"));
  assert.ok(!driver.includes("--virtual-time-budget"));
});

test("exact reviewed package must initialize twice in Chromium", () => {
  for (const token of [
    "for attempt in 1 2",
    'attempt_dir="${RUNNER_TEMP}/canonical-interface-browser/attempt-${attempt}"',
    "CANONICAL_BROWSER_ARTIFACT_DIR=\"$attempt_dir\"",
    "bash tests/interface-wasm-browser/run.sh",
  ]) {
    assert.ok(workflow.includes(token), `CI must retain ${token}`);
  }
});
