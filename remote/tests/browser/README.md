# Browser E2E suite

Hermetic browser end-to-end tests for k8s-cluster's front-end features. Unlike
the `../ui-*-smoke.mjs` scripts (which hit a live deployment), everything here
serves the **real in-repo asset** under test from a throwaway `127.0.0.1`
server and drives it with a headless browser — no deployment and no database.
Node's built-in `node:test` is the runner.

Every scenario runs under **both Puppeteer and Playwright** so a feature is
proven to behave identically on each engine. The harness blocks non-loopback
HTTP(S)/WebSocket traffic by default so an accidental external fetch cannot
make a supposedly hermetic test flaky or leak test data.

## What's covered

| File | Feature under test | Real asset |
| --- | --- | --- |
| `harness-hardening.test.mjs` | browser wrapper event semantics, external-network blocking, failure screenshots/diagnostics/traces, static-server method policy | `browser/harness.mjs` |
| `service-worker.test.mjs` | `dd-browser-drafts` offline draft cache — save / load / delete + error paths, over the worker's postMessage protocol | `remote/libs/browser/service-worker.js` |
| `func-approx-ui.test.mjs` | `dd-func-approx` UI shell, dd-data-viz config badge, client-side sample generators, custom-JSON validation | `remote/deployments/func-approx-rs/ui.html` |

`harness.mjs` holds the shared static server, engine launchers, loopback-only
network policy, normalized browser methods, diagnostics, failure artifacts, and
`pollUntil`.

## Running locally

```sh
cd remote/tests
pnpm install
pnpm exec playwright install chromium
pnpm exec puppeteer browsers install chrome

# both engines:
node --test browser/*.test.mjs

# one engine only (what each CI matrix job does):
BROWSER_ENGINES=playwright node --test browser/*.test.mjs
BROWSER_ENGINES=puppeteer  node --test browser/*.test.mjs
```

`BROWSER_ENGINES` is a comma-separated allowlist (`puppeteer`, `playwright`);
unset means both.

Set `BROWSER_ARTIFACT_DIR=/tmp/dd-browser-artifacts` to retain failure
screenshots and diagnostics. Playwright failures also include a trace ZIP that
can be opened with `pnpm exec playwright show-trace <trace.zip>`.

External requests are blocked by default. A deliberately non-hermetic test must
opt in explicitly with `withPage(engine, callback, { allowExternalRequests:
true })` so network use is visible in review.

## CI

`.github/workflows/browser-e2e.yml` runs the public hermetic suite on every
matching pull request without requiring private credentials. The private
`remote/libs` service-worker scenarios run in a separate matrix job only when
`K8S_LIBS_DEPLOY_KEY` is available. Both jobs upload retained browser artifacts
on failure.

## Adding a test

Serve the real asset (add a route in the test's `routes`), then assert against
it through the `withPage(engine, …)` helper. Keep tests backend-free: prefer
features reachable from page load, `page.evaluate()`, and the asset's own
client-side code. If a feature needs a backend response, mock it at the network
layer rather than standing up the server. Keep external-network access disabled
unless the test is explicitly a live smoke test in a separate workflow.
