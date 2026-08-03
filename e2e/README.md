# t2v-web browser e2e (Playwright)

Browser end-to-end tests for the t2v-web MASH dashboard. They complement the
Rust `crates/web/tests/web_smoke.rs` router tests by running the page in a real
Chromium: they prove the vendored htmx script loads and **executes** under the
strict `script-src 'self'` CSP, that the live-stats websocket connects, and that
navigation works — things a headless router test can't observe.

## Run locally

```sh
# from the repo root: build the binary Playwright boots
cargo build --release --bin t2v-web

cd e2e
npm install
npx playwright install chromium
npm test            # playwright boots t2v-web on an in-memory-ish SQLite file
```

Playwright's `webServer` config launches `../target/release/t2v-web` against a
throwaway SQLite DB (the migrator self-provisions it), waits for `/healthz`, then
runs the specs. Point `T2V_WEB_BIN` at a debug build to skip the release compile:

```sh
cargo build --bin t2v-web
T2V_WEB_BIN=../target/debug/t2v-web npm test
```

No t2v-api is required: the dashboard GET routes and assets under test never call
it (only the interactive translate/TTS proxy would).

## CI

`.github/workflows/ci.yml` runs these in the `browser-e2e` job on every push/PR
and uploads the Playwright HTML report as an artifact.
