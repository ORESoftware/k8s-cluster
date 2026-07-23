// Contract tests for the superproject wiring: asserts .gitmodules stays complete
// and pinned to main, that readme/boundary docs classify every app, that
// .env.example keeps required (placeholder-only) knobs, and that the ops scripts
// keep destructive actions manual with dry-run/audit guardrails.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readdirSync, readFileSync } from "node:fs";
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

test("submodule declarations stay complete, pinned to main, and backed by gitlinks", () => {
  const modules = parseGitmodules();
  const paths = modules.map((module) => module.path).sort();
  const gitlinks = new Map(
    execFileSync("git", ["ls-files", "--stage", "--", "apps"], {
      cwd: root,
      encoding: "utf8",
    })
      .trim()
      .split(/\r?\n/)
      .map((line) => {
        const entry = line.match(/^(\d+) [0-9a-f]+ \d+\t(.+)$/);
        assert.ok(entry, `unexpected git index entry: ${line}`);
        return [entry[2], entry[1]];
      }),
  );

  assert.equal(modules.length, 27);
  assert.deepEqual(paths, [
    "apps/fiducia-admin.rs",
    "apps/fiducia-ai-agent-bridge.rs",
    "apps/fiducia-ai-agent-control-plane",
    "apps/fiducia-ai-agent-manager.rs",
    "apps/fiducia-auth.rs",
    "apps/fiducia-brain.rs",
    "apps/fiducia-cli.rs",
    "apps/fiducia-clients",
    "apps/fiducia-customer.rs",
    "apps/fiducia-e2e",
    "apps/fiducia-edge",
    "apps/fiducia-infra",
    "apps/fiducia-interfaces",
    "apps/fiducia-lambda-service.rs",
    "apps/fiducia-load-balance.rs",
    "apps/fiducia-marketing.web",
    "apps/fiducia-mcp-server.rs",
    "apps/fiducia-memory.rs",
    "apps/fiducia-messaging.rs",
    "apps/fiducia-node-sidecar.rs",
    "apps/fiducia-node.rs",
    "apps/fiducia-operations-control-plane",
    "apps/fiducia-payments.rs",
    "apps/fiducia-routing.rs",
    "apps/fiducia-sync",
    "apps/fiducia-telemetry.rs",
    "apps/fiducia-test-config",
  ]);

  for (const module of modules) {
    assert.equal(module.branch, "main", `${module.path} must track main`);
    assert.match(module.url, /^git@github\.com:fiducia-cloud\/fiducia-[A-Za-z0-9.-]+(?:\.git)?$/);
    assert.ok(module.path.startsWith("apps/fiducia-"));
    assert.equal(gitlinks.get(module.path), "160000", `${module.path} must be a pinned gitlink`);
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

  assert.match(boundaries, /fiducia-monorepo` is the integration and GitOps superproject and is intended to\s+be private/);
  assert.match(boundaries, /Live visibility snapshot/);
  assert.match(boundaries, /Restore the all-up superproject to private unless an owner explicitly accepts/);
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
    "FIDUCIA_ADMIN_ORIGIN",
    "FIDUCIA_ADMIN_CSRF_SECRET",
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
    "FIDUCIA_ADMIN_CSRF_SECRET",
    "CUSTOMER_API_KEY_PEPPER",
    "SMTP_PASSWORD",
  ];

  for (const key of sensitiveKeys) {
    const value = env.get(key);
    assert.match(value, /replace[-_]me|secret-manager/i, `${key} must stay as a placeholder`);
    assert.doesNotMatch(value, /^eyJ[A-Za-z0-9_-]+\./, `${key} looks like a JWT`);
  }

  assert.match(env.get("FIDUCIA_ADMIN_ORIGIN"), /^https:\/\//);
  assert.ok(env.get("FIDUCIA_ADMIN_CSRF_SECRET").length >= 32);
});

test("monorepo scripts keep destructive actions manual and include dry-run/audit guardrails", () => {
  const scripts = readdirSync(path.join(root, "scripts"))
    .filter((file) => file.endsWith(".sh"))
    .sort();

  assert.deepEqual(scripts, [
    "audit-repo-state.sh",
    "check-interface-consumers.sh",
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
  const interfaceConsumers = read("scripts/check-interface-consumers.sh");
  assert.match(interfaceConsumers, /generated_interfaces_are_importable/);
  assert.match(interfaceConsumers, /RUSTC="\$rustc_bin"/);
  assert.match(interfaceConsumers, /public_consumers/);
  assert.match(interfaceConsumers, /private_consumers/);
  assert.match(audit, /:\(exclude\)target\/\*\*/);
  assert.match(audit, /:\(exclude\)node_modules\/\*\*/);
  assert.match(audit, /:\(exclude\)dist\/\*\*/);
  assert.match(audit, /GitHub Actions that are not pinned to full commit SHAs/);
  assert.match(audit, /\[0-9a-f\]\{40\}/);
  assert.match(audit, /sha256:\[0-9a-f\]\{64\}/);
  assert.match(audit, /fail-open or lockfile-bypassing workflow commands/);
  assert.match(audit, /runs npm dependency lifecycle scripts in a workflow/);
  assert.match(audit, /Cargo workflow commands without --locked/);
  assert.match(audit, /has no explicit top-level permissions/);
  assert.match(audit, /has no concurrency policy/);
  assert.match(audit, /does not bound every runner job/);
  assert.match(audit, /persists checkout credentials/);
  assert.match(audit, /uses a privileged untrusted-code trigger/);
  assert.match(audit, /publishes or consumes a mutable image tag/);
  assert.match(audit, /contains component-repository deployment commands/);
  assert.match(audit, /documents dependency-resolving Cargo commands without --locked/);
  assert.match(audit, /documents npm ci without disabling dependency lifecycle scripts/);
  assert.match(audit, /tracked directory.*is missing README\.md/);
  assert.match(audit, /git -C "\$repo" check-ignore --no-index/);
  assert.match(audit, /\[\[ \$\{#gitlinks\[@\]\} -gt 0 \]\]/);
  assert.match(audit, /container base images without immutable sha256 digests/);
  assert.match(audit, /package-ecosystem:\[\[:space:\]\]\*docker/);
  assert.match(audit, /tool-runner-nonroot/);
  assert.match(audit, /USER 65532:65532/);

  const branchScripts = [
    read("scripts/pin-submodules.sh"),
    read("scripts/checkout-feature-branch.sh"),
  ];
  for (const body of branchScripts) {
    assert.match(body, /\^\[A-Za-z0-9\._\/-\]\+\$/);
  }
  assert.match(branchScripts[0], /permits only the main branch/);
});

test("consensus services stay transport-independent from NATS", () => {
  const consensusRepos = [
    "apps/fiducia-brain.rs",
    "apps/fiducia-node.rs",
    "apps/fiducia-routing.rs",
    "apps/fiducia-load-balance.rs",
  ];

  for (const repo of consensusRepos) {
    const manifest = read(`${repo}/Cargo.toml`);
    assert.doesNotMatch(manifest, /\b(?:async-)?nats\b/i, `${repo} must not depend on NATS`);
    const sourceFiles = execFileSync("git", ["-C", path.join(root, repo), "ls-files", "src"], {
      encoding: "utf8",
    }).trim().split(/\r?\n/).filter((file) => file.endsWith(".rs"));
    for (const file of sourceFiles) {
      assert.doesNotMatch(read(`${repo}/${file}`), /\basync_nats\b/, `${repo}/${file} must use Raft peer transport, not NATS`);
    }
  }
});

test("CI and production promotion fail closed on immutable GitOps inputs", () => {
  const ci = read(".github/workflows/ci.yml");
  const deploy = read(".github/workflows/deploy.yml");
  const executableDeploy = deploy
    .split(/\r?\n/)
    .filter((line) => !line.trimStart().startsWith("#"))
    .join("\n");
  const dockerfile = read("Dockerfile");
  const dependabot = read(".github/dependabot.yml");

  for (const workflow of [ci, deploy]) {
    assert.match(workflow, /actions\/checkout@[0-9a-f]{40}/);
    assert.match(workflow, /actions\/setup-node@[0-9a-f]{40}/);
    assert.match(workflow, /FIDUCIA_SUBMODULE_TOKEN/);
    assert.match(workflow, /npm ci --ignore-scripts/);
    assert.doesNotMatch(workflow, /continue-on-error:\s*true|npm ci \|\| npm install/);
  }

  assert.match(ci, /Initialize public contract submodules/);
  for (const publicContractRepo of [
    "fiducia-interfaces",
    "fiducia-sync",
    "fiducia-brain.rs",
    "fiducia-node.rs",
    "fiducia-routing.rs",
    "fiducia-load-balance.rs",
  ]) {
    assert.match(ci, new RegExp(`apps/${publicContractRepo.replace(".", "\\.")}`));
  }
  assert.match(ci, /fleet-audit:/);
  assert.match(ci, /if: github\.event_name == 'workflow_dispatch'/);
  assert.match(ci, /submodules: recursive/);
  assert.match(read("package.json"), /node tools\/gitops-release\.mjs check/);
  assert.match(deploy, /environment: prod/);
  assert.match(deploy, /contents: write/);
  assert.match(deploy, /test -n "\$FIDUCIA_SUBMODULE_TOKEN"/);
  assert.match(deploy, /docker buildx imagetools inspect/);
  assert.match(deploy, /git -C "apps\/\$\{repo\}" rev-parse HEAD/);
  assert.match(deploy, /tools\/gitops-release\.mjs promote/);
  assert.match(deploy, /gh auth setup-git/);
  assert.match(deploy, /git push origin HEAD:main/);
  assert.doesNotMatch(executableDeploy, /KUBE_CONFIG|kubeconfig|kubectl[^\n]*apply|rollout status/);
  assert.match(dockerfile, /^FROM .*@sha256:[0-9a-f]{64}$/m);
  assert.match(dockerfile, /^USER 65532:65532$/m);
  assert.match(dependabot, /package-ecosystem: docker/);
});

test("Argo CD fans only the production data plane to the three providers", () => {
  const applicationSet = read("gitops/argocd/production-applicationset.yaml");
  const release = JSON.parse(read("gitops/release.json"));

  assert.match(applicationSet, /kind: AppProject/);
  assert.match(applicationSet, /kind: ApplicationSet/);
  assert.match(applicationSet, /fiducia\.cloud\/environment: production/);
  assert.match(applicationSet, /fiducia\.cloud\/plane: data/);
  assert.match(applicationSet, /values: \[hetzner, civo, vultr\]/);
  assert.match(applicationSet, /targetRevision: main/);
  assert.match(applicationSet, /gitops\/data-plane/);
  assert.match(applicationSet, /enabled: true/);
  assert.match(applicationSet, /prune: true/);
  assert.match(applicationSet, /selfHeal: true/);
  assert.doesNotMatch(applicationSet, /kind: Secret/);

  assert.equal(release.schema_version, 1);
  assert.deepEqual(release.clusters, ["civo", "hetzner", "vultr"]);
  for (const component of Object.values(release.components)) {
    assert.match(component.commit, /^[0-9a-f]{40}$/);
    assert.match(component.digest, /^sha256:[0-9a-f]{64}$/);
  }
  for (const cluster of release.clusters) {
    const manifest = read(`gitops/data-plane/${cluster}/manifests.yaml`);
    assert.doesNotMatch(manifest, /^kind:\s*Secret$/m);
    assert.doesNotMatch(manifest, /ghcr\.io\/fiducia-cloud\/[^\s]+:(?:latest|v\d)/);
    for (const component of Object.values(release.components)) {
      assert.match(manifest, new RegExp(`${component.image}@${component.digest}`));
    }
  }
});
