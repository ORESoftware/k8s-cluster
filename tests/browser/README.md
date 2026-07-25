# Browser e2e suites

End-to-end browser-automation smoke tests for the fabrication web server
(`dd-fabrication-web-server`, HTTP on `FABRICATION_WEB_PORT`, default `8115`).

Three framework-separate, self-contained npm packages — install and run each on
its own; none shares dependencies or config with the others:

| Suite | Runner | Browser provisioning |
| --- | --- | --- |
| [`selenium/`](selenium/) | `node --test` + `selenium-webdriver` | Selenium Manager drives your locally installed Chrome |
| [`playwright/`](playwright/) | `@playwright/test` | `npm run install:browsers` downloads Chromium |
| [`puppeteer/`](puppeteer/) | `node --test` + `puppeteer` | Chrome downloads automatically on `npm install` |

Cargo ignores this directory: only top-level `tests/*.rs` files (and
`tests/<dir>/main.rs`) are integration-test targets, and there is no Rust here.

## Shared contract

Every suite asserts the same three things, so a behavior drift shows up in all
frameworks identically:

1. `GET /healthz` → `200` with `{ "ok": true, "service": "dd-fabrication-web-server" }`
   (liveness is unconditional once the process serves HTTP).
2. `GET /readyz` → JSON with an `ok` boolean; `200` when persistence is ready or
   disabled, `503 {"ok":false}` when it is unavailable. The tests accept both
   statuses but require the body to be well-formed.
3. `GET /` (the operator surface; redirects to `/mash`) **rejects anonymous
   browsers** with `401`/`403`. The Rust server is the only authorization
   boundary in this fleet — an anonymous browser receiving operator content is
   an incident, so this assertion is load-bearing, not decorative.

## Running

Start the server first (from the repo root):

```sh
cargo run --bin dd-fabrication-web-server
```

Then, in any suite directory:

```sh
npm install          # once; playwright also needs: npm run install:browsers
npm test
```

Point the suites at a non-default host/port with:

```sh
DD_FAB_E2E_BASE_URL=http://127.0.0.1:8115 npm test
```

All browsers run headless. Nothing here authenticates: the suites deliberately
exercise only the anonymous surface (`/healthz`, `/readyz`) plus the
anonymous-rejection contract. Authenticated operator flows need a Supabase JWT
and belong in a later iteration.
