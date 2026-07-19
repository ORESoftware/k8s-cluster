import assert from "node:assert/strict";
import { readFileSync, statSync } from "node:fs";
import test from "node:test";

const workflow = readFileSync(new URL("../.github/workflows/ci.yml", import.meta.url), "utf8");
const audit = readFileSync(new URL("../scripts/audit-umbrella.sh", import.meta.url), "utf8");
const build = readFileSync(new URL("../build.sh", import.meta.url), "utf8");

test("CI is read-only, bounded, and runs for changes and drift", () => {
  assert.match(workflow, /push:\s*\n\s+branches:\s*\n\s+- main/);
  assert.match(workflow, /pull_request:/);
  assert.match(workflow, /workflow_dispatch:/);
  assert.match(workflow, /schedule:/);
  assert.match(workflow, /permissions:\s*\n\s+contents: read/);
  assert.match(workflow, /timeout-minutes: 20/);
  assert.doesNotMatch(workflow, /packages: write|contents: write|id-token: write|kubectl|kubeconfig/i);
});

test("every action is immutable and checkout includes the MCP gitlink", () => {
  const actionUses = [...workflow.matchAll(/^\s*uses:\s*([^\s#]+).*$/gm)].map((match) => match[1]);
  assert.ok(actionUses.length > 0, "workflow must use at least one action");
  for (const action of actionUses) {
    assert.match(action, /^[^@\s]+@[0-9a-f]{40}$/, `${action} must use a full commit SHA`);
  }
  assert.match(workflow, /persist-credentials: false/);
  assert.match(workflow, /submodules: recursive/);
  assert.match(workflow, /fetch-depth: 0/);
});

test("CI executes the auditable policy and locked MCP build", () => {
  assert.match(workflow, /\.\/scripts\/audit-umbrella\.sh/);
  assert.match(workflow, /node --test tests\/\*\.test\.mjs/);
  assert.match(
    workflow,
    /cargo test --locked --manifest-path canonical-mcp-server\.rs\/Cargo\.toml/,
  );
  assert.ok((statSync(new URL("../scripts/audit-umbrella.sh", import.meta.url)).mode & 0o111) !== 0);
});

test("umbrella audit covers boundaries, conflicts, secrets, and the remote MCP pin", () => {
  for (const expected of [
    "canonical-monorepo/build.sh",
    "canonical-web-server.rs/Cargo.toml",
    "canonical-marketing-site.web/package.json",
    "canonical-interfaces/package.json",
    "canonical-mcp-server.rs/Cargo.toml",
  ]) {
    assert.ok(audit.includes(expected), `missing boundary check for ${expected}`);
  }
  assert.match(audit, /unresolved merge markers found/);
  assert.match(audit, /tracked credential-like filenames found/);
  assert.match(audit, /high-confidence secret material found/);
  assert.match(audit, /git ls-files --stage/);
  assert.match(audit, /git ls-remote/);
  assert.match(audit, /canonical-mcp-server\.rs\.git/);
});

test("root build contract keeps customer, revoker, and client processes distinct", () => {
  assert.match(build, /npm ci && npm run typecheck && npm test && npm run build/);
  assert.match(build, /cargo build --locked --release --workspace --bins/);
  assert.match(build, /canonical-web-server serve/);
  assert.match(build, /canonical-session-revoker run/);
  assert.doesNotMatch(build, /service_role/i);
});
