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
    if (!trimmed || trimmed.startsWith("#")) {
      continue;
    }

    const eq = trimmed.indexOf("=");
    assert.notEqual(eq, -1, `env line is missing '=': ${line}`);
    env.set(trimmed.slice(0, eq), trimmed.slice(eq + 1));
  }

  return env;
}

test("submodule declarations stay complete, pinned to main, and backed by apps directories", () => {
  const modules = parseGitmodules();
  const paths = modules.map((module) => module.path).sort();

  assert.equal(modules.length, 16);
  assert.deepEqual(paths, [
    "apps/fiducia-admin.rs",
    "apps/fiducia-auth.rs",
    "apps/fiducia-backend.rs",
    "apps/fiducia-brain.rs",
    "apps/fiducia-cli.rs",
    "apps/fiducia-clients",
    "apps/fiducia-customer-ui.web",
    "apps/fiducia-edge",
    "apps/fiducia-infra",
    "apps/fiducia-interfaces",
    "apps/fiducia-load-balance.rs",
    "apps/fiducia-node-sidecar.rs",
    "apps/fiducia-node.rs",
    "apps/fiducia-routing.rs",
    "apps/fiducia-telemetry.rs",
    "apps/fiducia-ui.web",
  ]);

  for (const module of modules) {
    assert.equal(module.branch, "main", `${module.path} must track main`);
    assert.match(module.url, /^git@github\.com:fiducia-cloud\/fiducia-[A-Za-z0-9.-]+(?:\.git)?$/);
    assert.ok(module.path.startsWith("apps/fiducia-"));
    assert.ok(existsSync(path.join(root, module.path)), `${module.path} is initialized`);
  }
});

test("readme and boundary docs classify every app submodule", () => {
  const readme = read("readme.md");
  const boundaries = read("docs/repo-boundaries.md");

  for (const module of parseGitmodules()) {
    const repoName = path.basename(module.path);
    assert.match(readme, new RegExp(`\`${module.path.replaceAll(".", "\\.")}\``));
    assert.match(boundaries, new RegExp(`\`${repoName.replaceAll(".", "\\.")}\``));
  }

  assert.match(boundaries, /Keep the all-up superproject private/);
  assert.match(boundaries, /Do not commit real `\.env\*` files/);
});

test("env template exposes auth, passkey, 2fa, api key, email, and idempotency knobs", () => {
  const env = parseEnvExample();
  const required = [
    "SUPABASE_URL",
    "SUPABASE_AUTH_ISSUER",
    "SUPABASE_AUTH_AUDIENCE",
    "SUPABASE_PUBLISHABLE_KEY",
    "SUPABASE_SECRET_KEY",
    "CUSTOMER_PORTAL_URL",
    "SUPABASE_AUTH_PASSWORD_ENABLED",
    "SUPABASE_AUTH_EMAIL_OTP_ENABLED",
    "SUPABASE_AUTH_TOTP_ENABLED",
    "SUPABASE_AUTH_PASSKEY_ENABLED",
    "WEBAUTHN_RP_ID",
    "WEBAUTHN_ORIGIN",
    "TOTP_ISSUER",
    "CUSTOMER_API_KEY_PEPPER",
    "IDEMPOTENCY_KEY_HEADER",
    "IDEMPOTENCY_STORE_URL",
    "IDEMPOTENCY_RECORD_TTL_SECONDS",
    "EMAIL_FROM",
    "SMTP_HOST",
    "SMTP_PASSWORD",
  ];

  for (const key of required) {
    assert.ok(env.has(key), `.env.example is missing ${key}`);
    assert.notEqual(env.get(key), "", `${key} must not be blank`);
  }

  assert.equal(env.get("IDEMPOTENCY_KEY_HEADER"), "Idempotency-Key");
  assert.equal(env.get("SUPABASE_AUTH_TOTP_ENABLED"), "true");
  assert.equal(env.get("SUPABASE_AUTH_PASSKEY_ENABLED"), "true");
});

test("env template keeps sensitive values placeholder-only", () => {
  const env = parseEnvExample();
  const sensitiveKeys = [
    "SUPABASE_SECRET_KEY",
    "CUSTOMER_API_KEY_PEPPER",
    "SMTP_PASSWORD",
  ];

  for (const key of sensitiveKeys) {
    const value = env.get(key);
    assert.match(value, /replace[-_]me|secret-manager/i, `${key} must stay as a placeholder`);
    assert.doesNotMatch(value, /^eyJ[A-Za-z0-9_-]+\./, `${key} looks like a JWT`);
  }
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

  const branchScripts = [
    read("scripts/pin-submodules.sh"),
    read("scripts/checkout-feature-branch.sh"),
  ];
  for (const body of branchScripts) {
    assert.match(body, /\^\[A-Za-z0-9\._\/-\]\+\$/);
  }
});
