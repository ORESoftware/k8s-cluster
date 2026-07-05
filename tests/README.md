# Browser end-to-end tests

Both **Playwright** and **Puppeteer** drive a real headless Chromium **through
the onion proxy** to surf a local origin, and exercise the web dashboard. The
shared harness ([harness/overlay.mjs](harness/overlay.mjs)) compiles the release
binary if needed, generates three relay keys, boots a 3-relay overlay + the
SOCKS/UI client, and starts a local origin server — all on dynamically allocated
ports.

## What the tests prove

- **Traffic really goes through the overlay.** The browser is launched with
  `--proxy-server=socks5://…` **and** `--proxy-bypass-list=<-loopback>` (so even
  localhost is proxied). After navigating, the suite asserts the client's
  `/api/status` `circuits_built` counter grew by the number of navigations — the
  browser could not have reached the origin except through the onion client.
- **DNS goes through the proxy.** Pages are loaded via the `localhost` hostname
  (not an IP), so Chromium's SOCKS5 remote DNS resolves at the exit.
- **Caches are busted every load.** Each navigation uses a fresh browser context
  with the HTTP cache disabled (`page.setCacheEnabled(false)` /
  `Network.setCacheDisabled`) and a unique cache-buster query. The origin counts
  hits per path and forbids caching; the suite asserts the hit count grew by the
  number of navigations.
- **The dashboard works.** Status cards populate, the "browse through the onion
  network" form returns HTTP 200 through a circuit, and rendered markdown docs
  are served.

## Run

```sh
cd tests
npm install
npm run setup          # downloads Playwright's Chromium (Puppeteer brings its own)
npm test               # runs Puppeteer then Playwright
# or individually:
npm run test:puppeteer
npm run test:playwright
```

The Rust binary is built automatically on first run if `target/release/tor-server`
is missing. Relays run with `TOR_EXIT_ALLOW_PRIVATE=1` so the exit may reach the
loopback origin — that flag is for local testing only (see
[security](../docs/security.md)).

## Optional: surf the real internet

Set `TOR_E2E_REAL=1` to additionally hit a public site over the SOCKS proxy
(requires an exit with internet egress).
