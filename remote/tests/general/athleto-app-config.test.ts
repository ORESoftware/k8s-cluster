import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import test from "node:test";

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), "..", "..")]) {
    if (existsSync(resolve(candidate, "remote/argocd/dd-next-runtime/kustomization.yaml"))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();
const vendorRoot = "remote/deployments/athleto-app-rs";

// Vendored athleto-app-rs is a SECONDARY submodule checkout; skip content
// assertions when it is not initialized. The .gitmodules pin and the
// superproject argocd/gateway wiring assertions always run.
const vendorPresent = existsSync(resolve(repoRoot, vendorRoot, "Cargo.toml"));
const skipIfAbsent = vendorPresent
  ? false
  : `${vendorRoot} submodule not checked out; skipping vendored-source assertions`;

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), "utf8");
}

test("athleto-app-rs is pinned as an athlet-o org submodule", async () => {
  const gitmodules = await readRepoFile(".gitmodules");

  assert.match(gitmodules, /path = remote\/deployments\/athleto-app-rs/);
  assert.match(gitmodules, /url = git@github\.com:athlet-o\/athleto-app-rs\.git/);
  assert.match(
    gitmodules,
    /\[submodule "remote\/deployments\/athleto-app-rs"\][\s\S]*?branch = main/,
  );
});

test("app Cargo.toml declares the MASH stack (Maud + Axum + SeaORM + Supabase + HTMX)", { skip: skipIfAbsent }, async () => {
  const cargo = await readRepoFile(`${vendorRoot}/Cargo.toml`);

  assert.match(cargo, /description\s*=\s*".*Maud \+ Axum \+ SeaORM \+ Supabase \+ HTMX"/);
  assert.match(cargo, /maud\s*=/);
  assert.match(cargo, /axum\s*=\s*\{[^}]*features\s*=\s*\[[^\]]*"ws"[^\]]*\]/);
  assert.match(cargo, /sea-orm\s*=/);
  // Application queries use SeaORM; reqwest+rustls drives the Supabase REST API.
  assert.match(cargo, /reqwest\s*=/);
});

test("app retains its numbered schema audit trail", { skip: skipIfAbsent }, async () => {
  const migration1 = await readRepoFile(`${vendorRoot}/migrations/0001_products_and_carts.sql`);
  assert.ok(migration1.length > 0, "expected first migration to be non-empty");
  assert.ok(
    existsSync(resolve(repoRoot, vendorRoot, "migrations/0007_security_hardening.sql")),
    "expected the security-hardening migration to exist",
  );
  assert.ok(
    existsSync(resolve(repoRoot, vendorRoot, "migrations/0008_rls_products_carts.sql")),
    "expected the RLS migration to exist",
  );
});

test("app router exposes the storefront/auth/ws/health routes we care about", { skip: skipIfAbsent }, async () => {
  const lib = await readRepoFile(`${vendorRoot}/src/lib.rs`);

  assert.match(lib, /\.route\("\/",\s*get\(/);
  assert.match(lib, /\.route\("\/login",\s*get\(/);
  assert.match(lib, /\.route\("\/ws",\s*get\(/);
  assert.match(lib, /\.route\("\/healthz",\s*get\(/);
});

test("security middleware enforces CSRF and stamps hardened security headers", { skip: skipIfAbsent }, async () => {
  const security = await readRepoFile(`${vendorRoot}/src/security.rs`);

  assert.match(security, /CSRF_COOKIE:\s*&str\s*=\s*"athleto_csrf"/);
  assert.match(security, /CSRF_HEADER:\s*&str\s*=\s*"x-csrf-token"/);
  assert.match(security, /CONTENT_SECURITY_POLICY/);
  assert.match(security, /default-src 'self'/);
  assert.match(security, /X_FRAME_OPTIONS,\s*HeaderValue::from_static\("DENY"\)/);
  assert.match(security, /X_CONTENT_TYPE_OPTIONS,\s*HeaderValue::from_static\("nosniff"\)/);
});

test("vendored htmx asset is served same-origin with a javascript content-type", { skip: skipIfAbsent }, async () => {
  const assets = await readRepoFile(`${vendorRoot}/src/assets.rs`);

  assert.match(assets, /HTMX_JS_PATH:\s*&str\s*=\s*"\/static\/htmx/);
  assert.match(assets, /CONTENT_TYPE,\s*"text\/javascript;\s*charset=utf-8"/);
});

// Superproject wiring — these files are tracked directly in k8s-cluster (not in
// the submodule), so they always run. They assert the TRUE current state, which
// intentionally documents where athleto is and is NOT yet wired.
test("dd-next-runtime argocd wires the athleto app on its own athleto.store hosts", async () => {
  const kustomization = await readRepoFile("remote/argocd/dd-next-runtime/kustomization.yaml");
  const ingress = await readRepoFile("remote/argocd/dd-next-runtime/dd-athleto-app-rs.ingress.yaml");
  const jelloService = await readRepoFile("remote/argocd/dd-next-runtime/jello-ws.service.yaml");

  for (const resource of [
    "dd-athleto-app-rs.deployment.yaml",
    "dd-athleto-app-rs.ingress.yaml",
    "dd-athleto-app-rs.service.yaml",
    "jello-ws.service.yaml",
  ]) {
    assert.match(kustomization, new RegExp(resource.replace(/\./g, "\\.")));
  }

  // The dedicated Ingress routes both public hosts at the jello-ws alias :8145.
  assert.match(ingress, /host:\s*app\.athleto\.store/);
  assert.match(ingress, /host:\s*biz\.athleto\.store/);
  assert.match(ingress, /name:\s*jello-ws/);
  assert.match(ingress, /number:\s*8145/);
  assert.match(jelloService, /name:\s*jello-ws/);
  assert.match(jelloService, /port:\s*8145/);
});

test("standalone backend is reconciled internally while shared /jello still targets web-home-rs", async () => {
  const gateway = await readRepoFile("remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml");
  const kustomization = await readRepoFile("remote/argocd/dd-next-runtime/kustomization.yaml");

  // Truthful current state: the shared gateway's /jello locations proxy to the
  // legacy web-home-rs service, NOT to any athleto upstream. Do not invent
  // wiring that does not exist.
  assert.match(gateway, /location = \/jello \{/);
  assert.match(gateway, /set \$dd_up_3 dd-remote-web-home\.default\.svc\.cluster\.local:8080;/);
  assert.doesNotMatch(gateway, /athleto/i);

  // The standalone backend now runs as an internal workload from its source-
  // owned submodule manifests. Public /jello routing remains unchanged until
  // an explicit product cutover is reviewed.
  assert.match(kustomization, /deployments\/athleto-backend-rs\/k8s\/ec2/);
});
