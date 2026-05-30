# `dd-des-rs`

HTTP server that imports the **`discrete-event-system.rs` Rust engine as a
library** (via the `remote/submodules/discrete-event-system.rs` git submodule),
runs its simulation catalogue on demand, and serves the HTML result pages the
simulations render.

This is distinct from `des-simulator-rs` (`dd-des-simulator`), which ships its
own generic event-queue engine and serves the *TypeScript* DES submodule's
pre-committed `out/`. `dd-des-rs` runs the **real Rust engine** in-process.

## How it works

- `des_engine = { path = "../../submodules/discrete-event-system.rs" }` pulls in
  the engine crate. The catalogue (`des_engine::des::simulations`) exposes ~59
  simulations, each a `pub fn run()`.
- The engine writes artifacts (`out/*.html`, `out/*-framework.json`, JSONL
  frames, …) **relative to the process CWD**, so on startup the server `chdir`s
  into a writable working directory (`DES_WORK_DIR`, default a per-process temp
  dir) and serves `<work>/out`.
- Simulations run **strictly in series** behind a single lock — the engine uses
  a process-global clock/RNG and prints its report, so concurrent runs would
  race and interleave (mirroring the engine's own serial driver).
- At startup a fast, HTML-producing subset is rendered (`DES_STARTUP_SIMS`),
  ending with `main_build_site` which assembles the curated `out/index.html`.

## HTTP API

- `GET  /healthz` — readiness/liveness probe.
- `GET  /` — service info + endpoint map.
- `GET  /simulations` — the engine's full simulation catalogue.
- `POST /simulate` — run sims whose name contains `name`, e.g.
  `{"name":"electric_circuit"}`. Returns per-sim `{name, ok, millis}`.
- `GET  /simulations/<name>/run` — convenience GET form of `/simulate`.
- `GET  /out` → `/out/` — curated `index.html` if present, else a generated
  listing of every rendered artifact.
- `GET  /out/<path>` — serve an individual artifact (path-traversal confined to
  the `out/` directory via canonicalized checks).
- `GET  /docs/api`, `GET /api/docs`, `GET /api/docs.json` — generated API docs.

## Environment

| Var | Default | Meaning |
| --- | --- | --- |
| `HOST` | `0.0.0.0` | bind host |
| `PORT` | `8112` | bind port |
| `DES_WORK_DIR` | `<tmp>/dd-des-rs` | writable dir; engine renders into `<dir>/out` |
| `DES_STARTUP_SIMS` | curated fast set | comma-separated name filters run at startup (empty = skip) |

## Run locally

```bash
cd remote/deployments/des-rs
cargo run --release
# then:
curl localhost:8112/simulations
curl -X POST localhost:8112/simulate -H 'content-type: application/json' -d '{"name":"electric_circuit"}'
open http://localhost:8112/out/
```

## Deploy

Runs in the `dd-next-runtime` overlay on the stock `rust:1.90-bookworm` image
via `cargo run --release` against the repo mounted read-only at
`/opt/dd-next-1`, with `CARGO_*`/`HOME`/`DES_WORK_DIR` pointed at the writable
`/tmp` emptyDir. See `remote/argocd/dd-next-runtime/dd-des-rs.deployment.yaml`
and `dd-des-rs.service.yaml` (registered in that overlay's `kustomization.yaml`).

> The submodule pins the engine's `origin/main`. Bump the pointer
> (`git -C remote/submodules/discrete-event-system.rs pull && git add` the
> submodule) to deploy a newer engine revision.
