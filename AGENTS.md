# Agent guidelines — akrion-web-server.rs

Rust (axum + maud) web portal for **Akrion Sim**. Renders htmx pages, a portal
UI, WebSocket stats, and Supabase browser login. `akrion-backend.rs` owns the
realtime game/simulation routes; this repo owns the web pages.

## Layout

- `src/main.rs` — boot: tracing, state, router, bind, graceful shutdown.
- `src/app.rs` — app state, public browser config, env helpers, asset paths.
- `src/routes.rs` — axum routes, htmx partials, the WebSocket stream.
- `src/views.rs` — maud page templates and UI fragments.
- `src/data.rs` — dashboard stats and portal rows.
- `assets/` — `app.css`, `app.js`, `theme.js`, emblem.
- `e2e/` — Playwright + Puppeteer browser tests (`node --test`), isolated from cargo.

## Endpoints

- `GET /` — home page (`title` "Akrion Sim", hero, Home/Portal nav).
- `GET /healthz` — bare `ok` liveness probe.
- `GET /portal`, `GET /partials/*`, `GET /ws/portal` (WebSocket), `GET /config`.
- `/assets/*` — static files.

## Working here

- Enter the dev shell: `direnv allow` (or `nix develop ./.nix`, or `./shell`).
- Run: `PORT=8124 AKRION_BACKEND_URL=http://127.0.0.1:8113 cargo run`, open
  http://127.0.0.1:8124.
- Format + lint + test before pushing:
  ```sh
  cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
  ```
- Browser e2e: `cargo build` then `cd e2e && npm ci && npm test` (boots the
  built binary on an ephemeral port and drives it with Playwright and Puppeteer).

## Command safety

Agents working in this repo must **not** run destructive shell commands.

**Blacklisted (never run):** `rm`, `rm -rf`, `rmdir`, `dd`, `mkfs`, `shred`,
`truncate`, `> file` truncation, `find … -delete`, `git clean -fdx`,
`git reset --hard` on shared branches, `git push --force` to `main`, and any
`sudo`-prefixed or disk/format command.

**Whitelisted (prefer these):** `git rm` and `git mv` to delete/move tracked
files (reviewable and reversible via history), `git restore` / `git revert` to
undo, and scratch under the gitignored `tmp/`. When something must be removed,
stage it with `git rm` for review — never delete files out-of-band with `rm`.

## Git worktrees

Create git worktrees under `tmp/worktrees/`; `tmp/` is gitignored.
