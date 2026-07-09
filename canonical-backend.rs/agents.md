# Agent guidelines — canonical-backend.rs

Rust (axum) HTTP service for **canonical.cloud**. It serves the built Astro
site from `static/` and exposes a small JSON API under `/api`.

## Layout

- `src/main.rs` — the whole service: router, handlers, and unit tests.
- `static/` — the built Astro site (from `canonical-frontend`'s `dist/`).
  Not committed; injected at build/deploy time via `STATIC_DIR`.
- `.nix/flake.nix` — the dev shell (`nix develop`, wired up by `.envrc`).

## Endpoints

- `GET /healthz` — bare `200 OK` liveness probe (bypasses any gateway prefix).
- `GET /api/health` — `{ "status": "ok", "service": "canonical-backend" }`.
- `GET /api/info` — service, version, and domain metadata.
- everything else — served from `STATIC_DIR` (defaults to `static/`), with
  directory requests resolving to `index.html` and unknown paths falling back
  to the SPA index.

## Working here

- Enter the dev shell with `nix develop ./.nix` (or `direnv allow`, or `./shell`).
- Format + lint + test before pushing:
  ```sh
  cargo fmt
  cargo clippy --all-targets -- -D warnings
  cargo test --bins
  ```
  `cargo test` runs the in-binary unit tests in `src/main.rs` (this is a
  bin-only crate, so CI runs `cargo test --bins`).
- Run locally against the frontend build:
  ```sh
  STATIC_DIR=../canonical-frontend/dist PORT=8081 cargo run
  ```

## Git worktrees

Create git worktrees under `tmp/worktrees/` (e.g. `tmp/worktrees/<branch>`).
`tmp/` is gitignored, so worktree checkouts never show up as untracked files or
get committed by accident.

## Conventions

- Keep the API additive and JSON-shaped; probes must stay dependency-free so a
  degraded backend still reports liveness.
- No secrets in the repo. Runtime config comes from the environment
  (`PORT`, `STATIC_DIR`, `RUST_LOG`).
