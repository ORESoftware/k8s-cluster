import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";

const root = path.resolve(import.meta.dirname, "..");

function read(relPath) {
  return readFileSync(path.join(root, relPath), "utf8");
}

function parseGitmodules() {
  const modules = [];
  let current = null;

  for (const line of read(".gitmodules").split(/\r?\n/)) {
    const section = line.match(/^\[submodule "([^"]+)"\]$/);
    if (section) {
      current = { name: section[1] };
      modules.push(current);
      continue;
    }

    const field = line.match(/^\s*([^=]+?)\s*=\s*(.+)$/);
    if (field && current) {
      current[field[1]] = field[2];
    }
  }

  return modules;
}

function parseEnvExample() {
  const env = new Map();
  for (const line of read(".env.example").split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const eq = trimmed.indexOf("=");
    assert.notEqual(eq, -1, `env line is missing '=': ${line}`);
    env.set(trimmed.slice(0, eq), trimmed.slice(eq + 1));
  }
  return env;
}

test("submodule declarations stay complete, pinned to main, and backed by apps directories", () => {
  const modules = parseGitmodules();
  const paths = modules.map((module) => module.path).sort();

  assert.equal(modules.length, 2);
  assert.deepEqual(paths, ["apps/canonical-backend.rs", "apps/canonical-frontend"]);

  for (const module of modules) {
    assert.equal(module.branch, "main", `${module.path} must track main`);
    assert.match(
      module.url,
      /^git@github\.com:canonical-cloud\/canonical-[A-Za-z0-9.-]+(?:\.git)?$/,
      `${module.path} url must point at canonical-cloud over SSH`,
    );
    assert.ok(module.path.startsWith("apps/canonical-"));
    assert.ok(existsSync(path.join(root, module.path)), `${module.path} is initialized`);
  }
});

test("README and boundary docs classify every app submodule", () => {
  const readme = read("README.md");
  const boundaries = read("docs/repo-boundaries.md");

  for (const module of parseGitmodules()) {
    const repoName = path.basename(module.path);
    assert.match(readme, new RegExp(`\`${module.path.replaceAll(".", "\\.")}\``));
    assert.match(boundaries, new RegExp(`\`${repoName.replaceAll(".", "\\.")}\``));
  }

  assert.match(boundaries, /Do not commit real `\.env\*` files/);
});

test("env template exposes the runtime knobs and keeps values placeholder-only", () => {
  const env = parseEnvExample();
  for (const key of ["PORT", "STATIC_DIR", "RUST_LOG", "PUBLIC_BASE"]) {
    assert.ok(env.has(key), `.env.example is missing ${key}`);
    assert.notEqual(env.get(key), "", `${key} must not be blank`);
  }
  // No obvious real secrets in the template.
  assert.doesNotMatch(read(".env.example"), /ghp_[A-Za-z0-9]{36}/);
});

test("monorepo scripts keep destructive actions manual and include dry-run/audit guardrails", () => {
  const scripts = readdirSync(path.join(root, "scripts"))
    .filter((file) => file.endsWith(".sh"))
    .sort();

  assert.deepEqual(scripts, [
    "audit-repo-state.sh",
    "checkout-feature-branch.sh",
    "pin-submodules.sh",
  ]);

  for (const script of scripts) {
    const body = read(`scripts/${script}`);
    assert.ok(body.startsWith("#!/usr/bin/env bash\nset -euo pipefail\n"));
    assert.doesNotMatch(body, /\bgit\s+push\b/);
    assert.match(body, /--dry-run|--allow-dirty/);
  }

  const audit = read("scripts/audit-repo-state.sh");
  assert.match(audit, /:\(exclude\)target\/\*\*/);
  assert.match(audit, /:\(exclude\)node_modules\/\*\*/);
  assert.match(audit, /:\(exclude\)dist\/\*\*/);

  for (const body of [
    read("scripts/pin-submodules.sh"),
    read("scripts/checkout-feature-branch.sh"),
  ]) {
    assert.match(body, /\^\[A-Za-z0-9\._\/-\]\+\$/);
  }
});

test("every app agents.md blacklists rm and whitelists git rm / git mv", () => {
  for (const module of parseGitmodules()) {
    const agents = read(path.join(module.path, "agents.md"));
    assert.match(agents, /Command safety/, `${module.path}/agents.md needs a Command safety section`);
    assert.match(agents, /git rm/, `${module.path}/agents.md must whitelist git rm`);
    assert.match(agents, /git mv/, `${module.path}/agents.md must whitelist git mv`);
  }

  const own = read("agents.md");
  assert.match(own, /Command safety/);
  assert.match(own, /git rm/);
});
