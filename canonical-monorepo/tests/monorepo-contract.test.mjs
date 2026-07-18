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

  assert.equal(modules.length, 3);
  assert.deepEqual(paths, [
    "apps/canonical-interfaces",
    "apps/canonical-marketing-site.web",
    "apps/canonical-web-server.rs",
  ]);

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
  for (const key of [
    "PORT",
    "STATIC_DIR",
    "APP_ASSET_DIR",
    "RUST_LOG",
    "APP_BASE_URL",
    "APP_ALLOWED_ORIGINS",
    "APP_SESSION_ENCRYPTION_KEY",
    "LOGIN_RATE_LIMIT_ATTEMPTS",
    "LOGIN_RATE_LIMIT_GLOBAL_ATTEMPTS",
    "LOGIN_RATE_LIMIT_WINDOW_SECONDS",
    "LOGIN_RATE_LIMIT_MAX_KEYS",
    "LOGIN_AUTH_MAX_CONCURRENCY",
    "BEARER_AUTH_MAX_CONCURRENCY",
    "DATABASE_URL",
    "SESSION_REVOCATION_DATABASE_URL",
    "SESSION_REVOCATION_DATABASE_MAX_CONNECTIONS",
    "MIGRATION_DATABASE_URL",
    "MIGRATION_DATABASE_MAX_CONNECTIONS",
    "SUPABASE_URL",
    "SUPABASE_PUBLISHABLE_KEY",
    "PUBLIC_BASE",
  ]) {
    assert.ok(env.has(key), `.env.example is missing ${key}`);
    assert.notEqual(env.get(key), "", `${key} must not be blank`);
  }
  assert.equal(
    new Set([
      env.get("DATABASE_URL"),
      env.get("SESSION_REVOCATION_DATABASE_URL"),
      env.get("MIGRATION_DATABASE_URL"),
    ]).size,
    3,
    "web, revoker, and migration processes need distinct database credentials",
  );
  assert.match(env.get("DATABASE_URL"), /canonical_web_server/);
  assert.match(env.get("SESSION_REVOCATION_DATABASE_URL"), /canonical_session_revoker/);
  assert.equal(env.get("LOGIN_AUTH_MAX_CONCURRENCY"), "16");
  assert.equal(env.get("BEARER_AUTH_MAX_CONCURRENCY"), "32");
  assert.doesNotMatch(env.get("SUPABASE_PUBLISHABLE_KEY"), /service_role|secret/i);
  // No obvious real secrets in the template.
  assert.doesNotMatch(read(".env.example"), /ghp_[A-Za-z0-9]{36}/);
});

test("full-stack build includes both browser clients before all locked Rust workspace bins", () => {
  const build = read("build.sh");
  assert.match(build, /canonical-marketing-site\.web/);
  assert.match(build, /APP_CLIENT="\$WEB_SERVER\/client"/);
  assert.match(build, /npm run typecheck/);
  assert.match(build, /npm test/);
  assert.match(build, /npm run build/);
  assert.match(build, /cargo build --locked --release --workspace --bins/);
  assert.match(build, /canonical-web-server migrate/);
  assert.match(
    build,
    /unset MIGRATION_DATABASE_URL MIGRATION_DATABASE_MAX_CONNECTIONS SESSION_REVOCATION_DATABASE_URL SESSION_REVOCATION_DATABASE_MAX_CONNECTIONS/,
  );
  assert.match(build, /canonical-web-server serve/);
  assert.match(build, /canonical-session-revoker run/);
  assert.doesNotMatch(build, /\brm\b|\bcp\b/);
});

test("architecture docs keep migration, RLS, process, WebSocket, and backplane boundaries explicit", () => {
  const readme = read("README.md");
  const deploy = read("docs/deploy.md");
  const boundaries = read("docs/repo-boundaries.md");

  assert.match(readme, /canonical-web-server migrate/);
  assert.match(readme, /canonical-session-revoker run/);
  assert.match(deploy, /bootstrap_runtime_role\.sql/);
  assert.match(deploy, /bootstrap_session_revoker_role\.sql/);
  assert.match(deploy, /docker build --target web/);
  assert.match(deploy, /docker build --target revoker/);
  assert.match(deploy, /no-ingress/i);
  assert.match(deploy, /LISTEN.*NOTIFY/s);
  assert.match(deploy, /HTMX-owned WebSocket/);
  assert.match(boundaries, /non-owner, non-`BYPASSRLS`/);
  assert.match(boundaries, /services\/canonical-session-revoker/);
  assert.match(boundaries, /Administrative capabilities stay outside both deployed processes/);
});

test("stack smoke validates both the web process and isolated revoker startup", () => {
  const smoke = read("scripts/stack-smoke.sh");

  assert.match(smoke, /target\/release\/canonical-web-server/);
  assert.match(smoke, /target\/release\/canonical-session-revoker/);
  assert.match(smoke, /SESSION_REVOCATION_DATABASE_URL="sqlite::memory:"/);
  assert.match(smoke, /"\$REVOKER" check/);
  assert.doesNotMatch(smoke, /SESSION_REVOCATION_DATABASE_URL=.*"\$SERVER"/s);
});

test("pinned Rust service keeps bootstrap and runtime concerns modular", () => {
  const service = "apps/canonical-web-server.rs";
  const main = read(`${service}/src/main.rs`);
  const library = read(`${service}/src/lib.rs`);
  const manifest = read(`${service}/Cargo.toml`);
  const env = read(`${service}/.env.example`);
  const nonBlankLines = (source) => source.split(/\r?\n/).filter((line) => line.trim()).length;

  for (const suite of ["architecture.rs", "modularization.rs"]) {
    assert.ok(
      existsSync(path.join(root, service, "tests", suite)),
      `the deployable pin is missing the Rust ${suite} contract suite`,
    );
  }
  for (const module of ["app", "command", "database", "server", "telemetry"]) {
    assert.ok(existsSync(path.join(root, service, "src", `${module}.rs`)), `${module}.rs is missing`);
    assert.match(library, new RegExp(`^pub mod ${module};$`, "m"));
  }

  assert.ok(nonBlankLines(main) <= 24, "main.rs must remain a declarative bootstrap");
  assert.ok(nonBlankLines(library) <= 30, "lib.rs must remain a module map");
  assert.match(main, /telemetry::init/);
  assert.match(main, /command::run/);
  assert.doesNotMatch(main, /axum::|sea_orm|Router|TcpListener|ConnectOptions|Migrator/);
  assert.doesNotMatch(library, /pub struct|pub async fn|pub fn|impl /);

  assert.match(manifest, /^sea-orm\s*=/m);
  assert.doesNotMatch(manifest, /^sqlx\s*=/m);
  assert.doesNotMatch(env, /AUTO_MIGRATE|auto_migrate/);
  assert.match(read(`${service}/src/database.rs`), /pub async fn run_migrations/);
  assert.match(read(`${service}/src/server.rs`), /TcpListener::bind/);

  const telemetry = read(`${service}/src/telemetry.rs`);
  assert.match(telemetry, /OTEL_EXPORTER_OTLP_ENDPOINT/);
  assert.match(telemetry, /TraceContextPropagator/);
  assert.match(telemetry, /http\.server\.request\.count/);
  assert.match(telemetry, /http\.server\.request\.duration/);
  assert.match(telemetry, /with_writer\(std::io::stdout\)/);
  assert.match(telemetry, /resource_attribute_pairs/);
});

test("monorepo scripts keep destructive actions manual and include dry-run/audit guardrails", () => {
  const scripts = readdirSync(path.join(root, "scripts"))
    .filter((file) => file.endsWith(".sh"))
    .sort();

  assert.deepEqual(scripts, [
    "audit-repo-state.sh",
    "checkout-feature-branch.sh",
    "pin-submodules.sh",
    "stack-smoke.sh",
  ]);

  for (const script of scripts) {
    const body = read(`scripts/${script}`);
    assert.ok(body.startsWith("#!/usr/bin/env bash\n"));
    assert.match(body, /set -euo pipefail/);
    assert.doesNotMatch(body, /\bgit\s+push\b/);
  }

  // Scripts that touch git state must offer rehearsal/escape hatches; the
  // stack smoke is stricter — it must not run git at all.
  for (const script of ["audit-repo-state.sh", "checkout-feature-branch.sh", "pin-submodules.sh"]) {
    assert.match(read(`scripts/${script}`), /--dry-run|--allow-dirty/);
  }
  assert.doesNotMatch(read("scripts/stack-smoke.sh"), /\bgit\s/);

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

test("submodule pin verification fails closed when origin cannot be fetched", () => {
  const audit = read("scripts/audit-repo-state.sh");

  assert.match(
    audit,
    /else\s+fail "\$module_path: could not fetch origin\/\$module_branch to verify the pin"/,
  );
  assert.doesNotMatch(
    audit,
    /warn "\$module_path: could not fetch origin\/\$module_branch to verify the pin"/,
  );
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
