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
- `GET  /` — interactive landing page: per-simulation **Run** buttons (featured +
  full catalogue) that execute a sim in-process, plus a link to the rendered
  `out/` results. All of its `fetch`/links are relative, so it works both at `/`
  locally and behind the gateway at `/des-rs/`.
- `GET  /info` — service info + endpoint map (JSON).
- `GET  /simulations` — the engine's full simulation catalogue.
- `POST /simulate` — run sims by `name` (a *filter*; every sim whose name
  contains it runs), e.g. `{"name":"electric_circuit"}`. Pass `{"exact":true}`
  to run only the exactly-named entry. Returns per-sim `{name, ok, millis}`.
- `GET  /simulations/<name>/run` — convenience GET form of `/simulate`; add
  `?exact=1` to run exactly the named sim (this is what the UI buttons use).
- `GET  /models` — the first-class **model registry** (e.g. `mdp`, `pomdp`,
  `hybrid`, `studio`): each kind's descriptor (title, schema, solve methods,
  and a runnable example spec the UI/LLM can target).
- `GET  /models/<kind>/run` — run that kind's built-in example spec and render
  an interactive player; add `?format=json` for the raw artifact.
- `POST /models/<kind>/run` — run a user-supplied JSON spec for a kind (renders
  a player; `?format=json` returns the artifact). Validated, panic-isolated, and
  serialized behind the same lock as the simulations.
- `GET  /streaming` — the JSONL **streaming-solver** contracts (`lp`, `milp`,
  `mdp`, `pomdp`, `soccer-planner`): iterative solvers fed a JSONL command
  stream.
- `POST /streaming/<name>` — stream JSONL commands (one per line) to a solver;
  responds with a JSONL stream of result frames.
- `GET  /out/soccer-sim.html` — rendered 2D 11v11 soccer videogame / learning
  simulation artifact.
- `GET  /soccer/planner` — interactive soccer rotation planner UI.
- `POST /soccer/planner/solve` — re-solve the planner request with the Rust
  IP/MIP solver.
- `POST /soccer/planner/stream` — planner-specific JSONL stream alias for
  adding roster variables/constraints and solving.
- `GET  /music` — generative music-production workbench for microtonal albums
  and MP4 sample-seed variations.
- `POST /music/sample-seed` — upload a 10-50s MP4 seed plus prompt text and
  render a WAV variation with a JSON manifest.
- `GET  /delivery-planner.html` — friendly redirect to the generated delivery
  planner artifact.
- `GET  /deliver-planner.html` — typo-compatible redirect to the same generated
  delivery planner artifact.
- `GET  /out` → `/out/` — curated `index.html` if present, else a generated
  listing of every rendered artifact.
- `GET  /out/<path>` — serve an individual artifact (path-traversal confined to
  the `out/` directory via canonicalized checks).
- `GET  /api/docs.json` — the canonical **machine-readable service descriptor**
  (`des/service-descriptor/v1`). It is built by the engine library's
  `des::service` module (JSON-first) from this server's endpoints plus every
  registered extension's contributions (the engine simulation catalogue + this
  server's own `dd-des-rs-rendered-site` plugin).
- `GET  /docs/api`, `GET /api/docs` — a human-readable HTML **view** rendered
  independently by this server from that same descriptor (one source of truth,
  two representations).
- **Discovery:** `GET /` and `GET /info` return discovery response headers so a
  machine that hits only the canonical landing route can find the docs:
  `Link: <docs/api>; rel="service-doc", <api/docs.json>; rel="service-desc"`
  (RFC 8288 / RFC 8631) plus a first-party `dd-server-api-docs` header. The
  targets are relative, so they resolve to `/des-rs/docs/api` etc. behind the
  gateway. `GET /info` also echoes these under a `discovery` object.

## Environment

| Var | Default | Meaning |
| --- | --- | --- |
| `HOST` | `0.0.0.0` | bind host |
| `PORT` | `8112` | bind port |
| `DES_WORK_DIR` | `<tmp>/dd-des-rs` | writable dir; engine renders into `<dir>/out` |
| `DES_STARTUP_SIMS` | curated fast set | comma-separated name filters run at startup (empty = skip) |
| `DES_SERVER_GIT_URL` | k8s repo HTTPS URL | *(deployment startup script, not the server)* clone this repo at pod start and copy `remote/deployments/des-rs` from it; set empty to use the mounted repo |
| `DES_SERVER_GIT_REF` | `dev` | *(deployment startup script)* branch/tag/sha to clone for the server crate |
| `DES_ENGINE_GIT_URL` | engine repo HTTPS URL | *(deployment startup script, not the server)* clone this engine repo at pod start and build against it; set empty to use the pinned submodule instead |
| `DES_ENGINE_GIT_REF` | `0f96996384feeb3d5ac16f1494948e6d58799aab` | *(deployment startup script)* branch/tag/sha to clone for the engine |

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
via `cargo run --release`, with `CARGO_*`/`HOME`/`DES_WORK_DIR` pointed at the
writable `/tmp` emptyDir. See `remote/argocd/dd-next-runtime/dd-des-rs.deployment.yaml`
and `dd-des-rs.service.yaml` (registered in that overlay's `kustomization.yaml`).

**Engine source (auto-fetch).** At pod start the startup script fetches
`DES_ENGINE_GIT_REF` from `DES_ENGINE_GIT_URL` into `/tmp/engine`, copies this
crate into the writable `/tmp/des-rs`, repoints its `des_engine` path dependency
at the clone, and builds. The deployment currently pins the engine at
`0f96996384feeb3d5ac16f1494948e6d58799aab`, so it does **not** depend on the
node having the git submodule checked out (the push-time `reconcile-runtime`
only fast-forwards the repo; it does not `git submodule update --init`). If the
clone fails — or `DES_ENGINE_GIT_URL` is set empty — it falls back to the pinned
`remote/submodules/discrete-event-system.rs` submodule in the read-only repo
mount. To change engine source, set `DES_ENGINE_GIT_REF` to a branch/tag/sha or
clear `DES_ENGINE_GIT_URL` and bump the submodule pointer.

**Server source (auto-fetch).** The startup script also clones this k8s repo
(`DES_SERVER_GIT_URL` / `DES_SERVER_GIT_REF`) and copies
`remote/deployments/des-rs` from that fresh clone before building. This keeps
server routes in sync with Argo-applied manifests even when the EC2 hostPath
checkout mounted at `/opt/dd-next-1` has not been pulled yet. If the clone fails
or `DES_SERVER_GIT_URL` is empty, it falls back to the mounted repo.

The gateway exposes the landing page behind auth at **`/des-rs/`** (nginx
`location /des-rs/` → `dd-des-rs:8112/`, mirroring `/des/`), and the Rust
web-home service directory (`/home`) links to it as the `dd-des-rs` deployment /
`/des-rs/` row.

> The submodule pins the engine's `origin/main`. Bump the pointer
> (`git -C remote/submodules/discrete-event-system.rs pull && git add` the
> submodule) to deploy a newer engine revision.
