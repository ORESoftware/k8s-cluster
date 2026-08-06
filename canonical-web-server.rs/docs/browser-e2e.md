# Browser e2e — state, gaps, and how to shore it up

Puppeteer + Playwright end-to-end tests live in [`tests/browser/`](../tests/browser/)
and run via the `browser-e2e` CI job (ubuntu-latest, blocking) and the opt-in
`browser-e2e-selfhosted.yml` (self-hosted Chromium runners; see k8s-cluster
`docs/canonical-ci-runners-followups.md`).

The harness (`app-browser-harness.mjs`) compiles the binary once, runs the
explicit `migrate` command against a unique file-backed SQLite database, then
runs `serve` with `DATABASE_MAX_CONNECTIONS=1` against that same file. This is
the same privilege-separated boot recipe the `container-smoke` CI job proves.
No Postgres, Supabase, privileged runtime credential, or secrets are required.

**Covered today (8 tests):** login page render + form wiring, `/app` →
`/login` redirect, maud 404, `/api/v1/{health,info}` JSON contract + JSON 404,
app-vs-marketing CSP/security-header divergence, real form login through the
opaque session cookie and CSRF checks, engagements create/status/note htmx
swaps, IndexedDB-first offline writes and REST reconciliation, and
cookie-authenticated WebSocket invalidation/reconnect.

---

## 1. Authenticated product coverage (closed)

The blocking Playwright suite now enters through the real `/auth/login` form,
asserts the opaque `HttpOnly` session cookie, proves an invalid CSRF header is
rejected, and drives the engagements create/status/note lifecycle through htmx
swaps. A second authenticated scenario writes a draft while Chrome is offline,
observes the pending IndexedDB outbox, returns online, and waits for the REST
push to reconcile the record. It also observes an owner-scoped
`sync.invalidated` frame and forces a reconnect through the htmx WebSocket
extension.

The seam is the preferred provider swap: `test-auth` is a non-default Cargo
feature and it also requires `CANONICAL_TEST_AUTH_ENABLED=1`. `build.rs`
rejects it for the Cargo release profile even if that profile enables debug
assertions, while `src/lib.rs` independently rejects any profile without debug
assertions. CI runs the negative release build and checks the exact failure.
The production Dockerfile does not enable the feature, with a structural test
preserving these conditions. There is no session-minting endpoint, so the
browser exercises the same login, session encryption/storage, cookie, Origin,
and CSRF path used with Supabase.

The browser shell remains SQLite-backed. PostgreSQL owner isolation and RLS are
covered separately by `tests/postgres_rls.rs`; if browser behavior ever depends
on a PostgreSQL-only feature, add a dedicated Postgres browser job instead of
weakening this fast hermetic suite.

## 2. Dependency reproducibility in CI (closed)

`tests/browser/package-lock.json` is committed, and both hosted and self-hosted
jobs use `npm ci --prefix tests/browser`, so Playwright/Puppeteer resolution is
reproducible.

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

The browser suite does not contact Supabase or test Supabase's password service;
the `canonical-auth` integration tests retain responsibility for upstream
request shape, key scoping, token parsing, and error mapping.
