# daedalus e2e (browser)

Browser end-to-end tests for [daedalus-web-server](https://github.com/daedalus-fab/daedalus-web-server.rs),
focused on the **Supabase auth gate**, in both **Playwright** and **Puppeteer**.

## Why two tools

The requirement was coverage in both. They also fail differently: Playwright
drives its own Chromium/WebKit/Firefox with its own network interception;
Puppeteer drives headless Chrome via `setExtraHTTPHeaders`. Running the same
assertions through both catches a browser- or tool-specific difference either
alone would miss.

## What they assert

Without a token (always, when a base URL is set):
- `/health` is reachable and unauthenticated.
- `/` (the plans page) **refuses anonymous access** — `401` (auth on, no token)
  or `503` (auth unconfigured), **never `200`**. A `200` here is a failing auth
  gate, which is the thing this suite exists to catch.
- A forged bearer token does not authenticate.

With `DAEDALUS_WEB_TOKEN` (an allow-listed Supabase access token):
- `/` returns `200` and renders the "fabrication plans" heading.
- htmx is served from same-origin `/assets/htmx-*`, not a CDN (CSP invariant).

## Running

These tests are **env-gated** and skip when `DAEDALUS_WEB_BASE_URL` is unset, so
they never block base CI or require a browser download for `npm test` at the repo
root. To actually run them against a deployment:

```sh
cd tests/e2e
npm install                       # pulls @playwright/test + puppeteer
npx playwright install chromium   # Playwright browser binary

export DAEDALUS_WEB_BASE_URL=https://app.daedalus-fab.com
export DAEDALUS_WEB_TOKEN=<supabase-access-token>   # optional; unlocks authed tests

npm run test:playwright
npm run test:puppeteer
# or both:
npm test
```

A local target works too — run daedalus-web-server with auth configured and point
`DAEDALUS_WEB_BASE_URL` at `http://localhost:8115`.

## Relationship to base CI

The repo-root `npm test` runs a cheap contract test
([`../e2e-harness.test.mjs`](../e2e-harness.test.mjs)) that verifies this harness
stays wired (both specs present, both gating on the same env var, both asserting
the `200` boundary) without downloading a browser or needing a live server.
