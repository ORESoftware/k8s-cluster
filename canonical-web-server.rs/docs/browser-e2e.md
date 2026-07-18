# Browser e2e — state, gaps, and how to shore it up

Puppeteer + Playwright end-to-end tests live in [`tests/browser/`](../tests/browser/)
and run via the `browser-e2e` CI job (ubuntu-latest, blocking) and the opt-in
`browser-e2e-selfhosted.yml` (self-hosted Chromium runners; see k8s-cluster
`docs/canonical-ci-runners-followups.md`).

The harness (`app-browser-harness.mjs`) compiles the binary once and runs
`serve` on **in-memory SQLite** with `DATABASE_MAX_CONNECTIONS=1` +
`AUTO_MIGRATE=true` — the same boot recipe the `container-smoke` CI job proves.
No Postgres, Supabase, or secrets required.

**Covered today (6 tests, unauthenticated surface):** login page render + form
wiring, `/app` → `/login` redirect, maud 404, `/api/v1/{health,info}` JSON
contract + JSON 404, app-vs-marketing CSP/security-header divergence.

---

## 1. No authenticated coverage — the biggest gap (HIGH)

Everything behind `SessionAuthenticated` is browser-untested: the `/app`
dashboard, the engagements CRUD flow (`/app/engagements` create/status/notes),
the htmx fragments, and the offline-sync WebSocket loop (`/ws`). These are the
app's actual product surface, and the router tests in `tests/app.rs` cover them
only at the `oneshot` layer — not through a real browser (htmx swaps, IndexedDB,
optimistic UI, WS reconnect).

Why it's blocked: logging in needs a real Supabase password exchange. `tests/app.rs`
already solves the equivalent for router tests with a `FakeAuth` provider and by
minting sessions directly through `SessionService`.

**How to shore up.** Add an env-gated test-auth seam to the *binary* (not just
the test crate), so the e2e harness can obtain a session:
- Option A (preferred): a `#[cfg(feature = "test-auth")]` build that swaps
  `SupabaseAuth` for a fake provider accepting a fixed credential, gated behind
  an env flag that is impossible to set in prod images. Harness logs in via the
  real `/auth/login` form → exercises the whole cookie/CSRF path.
- Option B: a test-only signed-session minting endpoint compiled out of release
  builds. Faster but bypasses the login flow (less realistic).
Then seed a couple of `audit_engagement` rows and assert the dashboard, the
create-engagement htmx swap, and a note round-trip. Keep SQLite for the shell
tests; use the Postgres service (already in CI for `postgres-rls`) if a test
needs RLS-faithful behavior.

## 2. Dependency reproducibility in CI (MED)

`tests/browser/` ships **no lockfile** (`.gitignore` excludes it) and CI runs
`npm install`, so Playwright/Puppeteer float within the `^` ranges — a silent
version drift risk and a supply-chain surface. Marketing-site e2e, by contrast,
commits `package-lock.json` and uses `npm ci`.

**Fix:** commit `tests/browser/package-lock.json`, drop it from `.gitignore`,
and switch the CI step + `browser-e2e-selfhosted.yml` to `npm ci --prefix tests/browser`.

## 3. Double Chromium provisioning in CI (LOW, wasteful)

The `browser-e2e` job runs `playwright install --with-deps chromium` **and**
lets Puppeteer download its own Chrome during `npm install` (~two browser
downloads per run). The self-hosted image already unifies on one OS Chromium via
`PUPPETEER_EXECUTABLE_PATH`/`PLAYWRIGHT_CHROMIUM`.

**Fix:** on ubuntu-latest, set `PUPPETEER_SKIP_DOWNLOAD=1` and point both drivers
at the Playwright-managed Chromium (export `PUPPETEER_EXECUTABLE_PATH=$(node -e
"console.log(require('playwright').chromium.executablePath())")`). One download,
one browser, both engines.

## 4. Client-bundle assertion is a soft check (LOW)

`ensureClientBundle()` is best-effort: if `client/dist/app.js` is missing, the
login page's `<script type=module>` just 404s (no uncaught error), so
`app-playwright`'s page-error assertion still passes — a real "bundle didn't
build" regression would slip through locally. CI builds the client explicitly,
so it's covered there, but the local signal is weak.

**Fix:** when the bundle exists, additionally assert `GET /app-assets/app.js`
returns 200 with a JS content-type; only skip that assertion when the client
`node_modules` truly aren't installed, and `log()` the skip.

## 5. Harness robustness (LOW)

- `resolveBinary()` shells `cargo build` with `stdio: inherit` — fine locally,
  noisy in CI logs; consider `--message-format=short`.
- No explicit server-crash surfacing: if `serve` dies during boot, the only
  signal is the 60s `waitForReady` timeout. Capturing the child's stderr on
  failure would make CI failures diagnosable in one look.
- The `pageerror`-empty assertion won't catch failed *network* requests (e.g. a
  404 asset). A `page.on('requestfailed')` / `response` guard would tighten it.

## 6. What was NOT verified

The self-hosted path (`browser-e2e-selfhosted.yml` on `runs-on: canonical-browser`)
has never executed — the runner scale set isn't deployed. It is validated only as
YAML + by mirroring the working ubuntu-latest job. See the k8s-cluster followups
doc for the deploy checklist before trusting it.
