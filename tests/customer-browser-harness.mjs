// Repo-local boot recipe for the customer portal E2E.
//
// The genuinely-shared pieces (Chrome discovery + the server lifecycle + the
// stub Supabase) come from @fiducia/test-config; only the customer-specific
// boot arguments and the tiny stub fiducia-auth live here, next to the app
// they boot. Specs stay in this repo's tests/.
import { createServer } from "node:http";
import { existsSync } from "node:fs";
import { delimiter, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { startServer } from "@fiducia/test-config/harness";
import { startStubSupabase, verifyEs256Jwt } from "@fiducia/test-config/stubs";

export { chromeExecutablePath, launchOptions } from "@fiducia/test-config/harness";

const testsDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(testsDir, "..");
const CUSTOMER_SURFACE_AUDIENCE = "fiducia-customer";
const CUSTOMER_ROLE = "customer";
const CUSTOMER_CAPABILITY = "customer:self-service";
const AUTHORIZATION_CONTEXT_VERSION = 1;

/** The one account the stub Supabase accepts. `user_id` must be a UUID because
 * the store keys customer rows on it; the org matches the FIDUCIA_E2E static
 * customer so either auth mode admits the same tenant. Roles are trusted app
 * metadata, matching the only source accepted by the real fiducia-auth service. */
export const CUSTOMER = {
  id: "22222222-2222-4222-8222-222222222222",
  email: "dev@acme.com",
  password: "customer-pw",
  app_metadata: {
    orgs: ["00000000-0000-4000-8000-000000000001"],
    roles: [CUSTOMER_ROLE],
  },
};

/** The fixed email/SMS one-time code the stub Supabase accepts (stubs.mjs). */
export const STUB_OTP_CODE = "123456";

// Supabase and fiducia-auth are stubbed in-process, so the only external
// prerequisite for spawning the real server is its customer Postgres plane.
const requiredSpawnEnv = ["DATABASE_URL"];

// The app deliberately fails closed without its customer database. Reuse a
// fully configured server (one backed by the SAME stub contract, or the OTP
// journey cannot redeem the fixed code), or supply a database for the harness
// to spawn one; never weaken production startup for an E2E.
export function unavailableReason() {
  if (process.env.FIDUCIA_CUSTOMER_TEST_URL) return null;
  const missing = requiredSpawnEnv.filter((name) => !process.env[name]);
  return missing.length
    ? `set FIDUCIA_CUSTOMER_TEST_URL or configure: ${missing.join(", ")}`
    : null;
}

/**
 * The `cargo` to build/run this repo with. Prefer a rustup PROXY over whatever
 * `cargo` sits first on PATH: this repo pins its toolchain in
 * `rust-toolchain.toml` (1.97), and only the proxy honors that pin — a plain
 * distro/Homebrew cargo ignores it and fails the `rust-version` check.
 */
function cargoCommand() {
  const home = process.env.HOME ?? "";
  for (const candidate of [
    resolve(home, ".cargo/bin/cargo"), // rustup's default proxy location
    "/opt/homebrew/opt/rustup/bin/cargo", // Homebrew rustup (Apple Silicon)
    "/usr/local/opt/rustup/bin/cargo", // Homebrew rustup (Intel)
  ]) {
    if (candidate && existsSync(candidate)) return candidate;
  }
  return "cargo";
}

/** cargo shells out to `rustc` via PATH, so the proxy's dir must come first. */
function cargoEnv() {
  const cargo = cargoCommand();
  if (cargo === "cargo") return {};
  return { PATH: `${dirname(cargo)}${delimiter}${process.env.PATH ?? ""}` };
}

function normalizedCustomerAuthorization(claims) {
  const rawRoles = claims.app_metadata?.roles;
  if (
    !Array.isArray(rawRoles) ||
    rawRoles.length !== 1 ||
    rawRoles[0] !== CUSTOMER_ROLE
  ) {
    return null;
  }
  return {
    version: AUTHORIZATION_CONTEXT_VERSION,
    surface_audiences: [CUSTOMER_SURFACE_AUDIENCE],
    roles: [CUSTOMER_ROLE],
    capabilities: [CUSTOMER_CAPABILITY],
  };
}

/**
 * Stub fiducia-auth: exactly the `GET /v1/me` contract src/auth.rs consumes.
 * Verifies the presented bearer against the stub Supabase's public JWK — the
 * same trust chain as the real service, minus the org-sync machinery — and
 * emits the versioned customer authorization context from trusted app metadata.
 * A missing, unknown, or admin role fails closed rather than manufacturing a
 * customer audience in the test harness.
 */
function startStubFiduciaAuth(stubSupabase) {
  const server = createServer((req, res) => {
    const respond = (status, body) => {
      const payload = JSON.stringify(body);
      res.writeHead(status, { "content-type": "application/json" });
      res.end(payload);
    };
    const path = new URL(req.url, "http://stub").pathname;
    if (req.method === "GET" && path === "/healthz") {
      return respond(200, { ok: true });
    }
    if (req.method === "GET" && path === "/v1/me") {
      const bearer = (req.headers.authorization ?? "").replace(/^Bearer /, "");
      const claims = verifyEs256Jwt(stubSupabase.jwk, bearer);
      if (!claims || claims.exp <= Math.floor(Date.now() / 1000)) {
        return respond(401, { ok: false, error: "invalid_or_expired_session" });
      }
      const authorization = normalizedCustomerAuthorization(claims);
      if (!authorization) {
        return respond(403, { ok: false, error: "customer_surface_forbidden" });
      }
      return respond(200, {
        user: {
          user_id: claims.sub,
          email: claims.email ?? null,
          orgs: claims.app_metadata?.orgs ?? [],
          aal: claims.aal ?? "aal1",
          roles: authorization.roles,
          authorization,
        },
      });
    }
    respond(404, { ok: false, error: `stub-auth: no route for ${req.method} ${path}` });
  });
  return new Promise((resolvePromise, rejectPromise) => {
    server.once("error", rejectPromise);
    server.listen(0, "127.0.0.1", () => {
      resolvePromise({
        url: `http://127.0.0.1:${server.address().port}`,
        stop: () =>
          new Promise((resolveStop) => {
            server.closeAllConnections?.();
            server.close(() => resolveStop());
          }),
      });
    });
  });
}

// Boots the real fiducia-backend (axum) via `cargo run` against an in-process
// stub Supabase + stub fiducia-auth. The Rust build happens in the harness (no
// npm build step); FIDUCIA_SITE_MODE=customer serves the portal on loopback and
// the debug build derives its exact-origin/CSRF config from the chosen port.
export async function startCustomer() {
  if (process.env.FIDUCIA_CUSTOMER_TEST_URL) {
    return {
      url: process.env.FIDUCIA_CUSTOMER_TEST_URL.replace(/\/$/, ""),
      stop: async () => {},
    };
  }

  const supabase = await startStubSupabase({
    users: [CUSTOMER],
    orgs: [{ id: "00000000-0000-4000-8000-000000000001", plan: "pro" }],
  });
  let auth;
  let server;
  const stop = async () => {
    await server?.stop();
    await auth?.stop();
    await supabase.stop();
  };
  try {
    auth = await startStubFiduciaAuth(supabase);
    server = await startServer({
      command: cargoCommand(),
      args: ["run"],
      cwd: repoRoot,
      env: {
        ...cargoEnv(),
        SUPABASE_URL: supabase.url,
        SUPABASE_PUBLISHABLE_KEY: "stub-publishable-key",
        FIDUCIA_AUTH_URL: auth.url,
        FIDUCIA_SITE_MODE: "customer",
        // The canonical test schema is applied to PostgreSQL's public schema.
        // Production defaults to the isolated `fiducia` schema, so pinning this
        // fixture explicitly prevents a successful login from landing on a
        // database error page while preserving the production isolation default.
        FIDUCIA_DB_SCHEMA: "public",
        // Debug-only: emit non-Secure session/CSRF/MFA cookies so the browser
        // jar is inspectable over http://127.0.0.1 (both drivers filter Secure
        // cookies out of http origins). Mirrors the admin harness.
        FIDUCIA_INSECURE_COOKIES: "1",
      },
      readyPath: "/healthz",
      startupTimeoutMs: 300000,
    });
  } catch (error) {
    try {
      await stop();
    } catch (cleanupError) {
      throw new AggregateError(
        [error, cleanupError],
        "customer stack startup and cleanup failed",
      );
    }
    throw error;
  }
  return { url: server.url, stop };
}
