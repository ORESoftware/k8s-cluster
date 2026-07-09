# canonical-backend

Rust ([axum](https://github.com/tokio-rs/axum)) HTTP service for
**[canonical.cloud](https://canonical.cloud)**. It serves the built Astro
marketing site and exposes a small JSON API.

Part of the [`canonical-monorepo`](https://github.com/canonical-cloud/canonical-monorepo)
superproject; also usable standalone.

## Endpoints

| Method | Path          | Response                                                      |
| ------ | ------------- | ------------------------------------------------------------ |
| GET    | `/healthz`    | `200 OK` — bare liveness/readiness probe                     |
| GET    | `/api/health` | `{ "status": "ok", "service": "canonical-backend" }`         |
| GET    | `/api/info`   | `{ "service", "version", "domain" }`                         |
| GET    | `/*`          | static Astro site from `STATIC_DIR` (SPA-style index fallback) |

## Configuration

| Env var      | Default    | Purpose                                            |
| ------------ | ---------- | -------------------------------------------------- |
| `PORT`       | `8081`     | listen port                                        |
| `STATIC_DIR` | `static`   | directory of the built Astro site                  |
| `RUST_LOG`   | `info`     | `tracing` env-filter                               |

## Develop

This repo ships a Nix dev shell and an `.envrc`:

```sh
direnv allow          # or: nix develop ./.nix   (or: ./shell)
```

Build and run against the sibling frontend build:

```sh
cargo run                                   # serves ./static
STATIC_DIR=../canonical-frontend/dist cargo run
```

## Test

```sh
cargo test --bins     # in-binary unit tests (bin-only crate)
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

## Container

```sh
docker build -t canonical-backend .
docker run -p 8081:8081 -v "$PWD/static:/app/static:ro" canonical-backend
```
