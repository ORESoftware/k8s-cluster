# Browser E2E suite

Hermetic browser end-to-end tests for k8s-cluster's front-end features. Unlike
the `../ui-*-smoke.mjs` scripts (which hit a live deployment), everything here
serves the **real in-repo asset** under test from a throwaway `127.0.0.1`
server and drives it with a headless browser — no network, no deployment, no
database. Node's built-in `node:test` is the runner.

Every scenario runs under **both Puppeteer and Playwright** so a feature is
proven to behave identically on each engine.

## What's covered

| File | Feature under test | Real asset |
| --- | --- | --- |
| `service-worker.test.mjs` | `dd-browser-drafts` offline draft cache — save / load / delete + error paths, over the worker's postMessage protocol | `remote/libs/browser/service-worker.js` |
| `func-approx-ui.test.mjs` | `dd-func-approx` UI shell, dd-data-viz config badge, client-side sample generators, custom-JSON validation | `remote/deployments/func-approx-rs/ui.html` |

`harness.mjs` holds the shared bits: the static file server, the engine
launchers with a Puppeteer→Playwright-Chromium fallback, and `pollUntil`.

## Running locally

```sh
cd remote/tests
pnpm install
pnpm exec playwright install chromium      # one-time; Puppeteer fetches its own on install

# both engines:
node --test browser/service-worker.test.mjs browser/func-approx-ui.test.mjs

# one engine only (what each CI matrix job does):
BROWSER_ENGINES=playwright node --test browser/*.test.mjs
BROWSER_ENGINES=puppeteer  node --test browser/*.test.mjs
```

`BROWSER_ENGINES` is a comma-separated allowlist (`puppeteer`, `playwright`);
unset means both.

## CI

`.github/workflows/browser-e2e.yml` runs the suite as a matrix — a `puppeteer`
job and a `playwright` job — on pushes to `dev`/`main` and on PRs that touch
these tests or the assets they cover. It initialises only the `remote/libs`
submodule (the service worker lives there) and provisions Chromium + system
libraries via `playwright install --with-deps`.

## Adding a test

Serve the real asset (add a route in the test's `routes`), then assert against
it through the `withPage(engine, …)` helper. Keep tests backend-free: prefer
features reachable from page load, `page.evaluate()`, and the asset's own
client-side code. If a feature needs a backend response, mock it at the network
layer rather than standing up the server.
