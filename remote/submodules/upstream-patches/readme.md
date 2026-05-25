# Upstream patches

Self-contained `git apply`-style diffs targeting upstream submodule repositories.
Each patch in this folder is the unit of work that needs to be PR'd to the
upstream repo so this cluster repo can re-pin the submodule and stop carrying
the diff locally.

The cluster repo's submodule `working tree` will be tracked-dirty for as long
as a patch here represents code that hasn't merged upstream yet. That is
deliberate per `AGENTS.md` (surface dirty state, do not hide it). Once the
upstream PR merges and the submodule pointer in this repo is bumped, delete
the corresponding patch from this folder.

## Patches

### `lmx-node-slice-c-admin-panel.patch`

- Target submodule: `remote/submodules/live-mutex`
- Upstream repo:    `git@github.com:ORESoftware/live-mutex.git`
- Base branch:      `feat/sweeper-fencing-acquire-many-http` (this is what the
                    broker is pinned at in this cluster repo).
- Slice:            C in the
                    [admin-panel rollout plan](../../tests/lmx-admin-public-smoke.mjs)
                    documented inline at the top of that script. Sibling
                    of `lmx-rs-slice-b-admin-panel.patch`.

What it adds upstream:

- A new `src/log-level.ts` module exporting a process-global log-level
  filter (`error < warn < info < debug`) initialised from `LMX_LOG_LEVEL`.
  Public surface: `getLogLevel`, `setLogLevel`, `levelEnabled`, `listLevels`.
  The Node port deliberately advertises a discrete enum rather than the
  Rust port's `EnvFilter` directives because there's no equivalent
  filtering machinery in this codebase and bolting one on would be
  over-engineering.
- `broker-1.ts` rewires the existing `log` object so `log.warn`/`log.info`/
  `log.debug` consult the live level on every call. `log.error` is always
  emitted (errors are never silenced). The legacy `--lmx-debug` / `lmx_debug=yes`
  escape hatch still forces-on `log.debug` for backwards compatibility.
- `Broker1.isNoDelay()` / `Broker1.setNoDelay(bool)` accessor pair so the
  HTTP admin endpoint can read and flip the runtime TCP_NODELAY default
  applied to *newly* accepted connections. (Already-accepted sockets keep
  whichever setting they were configured with at accept time — same
  semantics as the Rust port.)
- New HTTP admin endpoints on `LMXHttpServer`, gated by the same
  `LMX_ADMIN_TOKEN` shared secret that already gates `/admin/otel`:
  - `GET /admin/log` — `{runtime: "node", level, levels[]}`
  - `POST /admin/log` — body `{level: "info"}`, returns
    `{previous, level, levels[]}`
  - `GET /admin/tcp` — `{runtime, tcp_nodelay, tcp_nodelay_supported: true,
    tcp_quickack: false, tcp_quickack_supported: false, notes}`. Node's
    `net` module doesn't expose `TCP_QUICKACK` (libuv doesn't wrap that
    setsockopt), so the endpoint advertises it as unsupported.
  - `POST /admin/tcp` — body `{tcp_nodelay?: bool}`. Any POST that
    includes `tcp_quickack` is rejected with 400 + a pointer to the
    `tcp_quickack_supported: false` field; we don't silently apply half
    a payload.
- Interactive HTML admin panel rendered from `http-server.ts`:
  - `localStorage`-backed admin-token input
  - OTel Enable / Disable buttons (delegates to the existing `/admin/otel`)
  - Log-level `<select>` populated from `listLevels()` + Apply button
  - `TCP_NODELAY` On / Off buttons
  - `TCP_QUICKACK` row rendered with `disabled` buttons + a
    "not supported on Node.js runtime" pill — visual parity with the
    Rust panel's non-Linux render path.
  - All controls are vanilla JS; no framework. Fetch calls send
    `x-admin-token` from `localStorage["lmx-admin-token"]`. Endpoints
    also accept `Authorization: Bearer <token>`.
  - Meta-refresh bumped from 5s to 30s so the admin panel state isn't
    wiped out from under the operator mid-edit.
- Test coverage: new `test/admin-log-tcp-toggle-test.ts` mirrors the
  shape of `admin-otel-toggle-test.ts` and asserts:
  - 401 on unauthenticated GET/POST to both endpoints
  - 200 GET reports current state + runtime support flags
  - 200 POST flips state, returns `previous`, in-process accessors
    (`getLogLevel`, `broker.isNoDelay()`) agree with the HTTP response
  - 400 on missing/wrong-type/unknown-level POSTs to `/admin/log`
  - 400 on empty body, wrong-type, or any attempt to set
    `tcp_quickack` against `/admin/tcp`
  - Both `x-admin-token` and `Authorization: Bearer` headers accepted
  - Wrong-token requests do not mutate state

What it does **not** change:

- Wire protocol — no new TCP framing, no client breakage.
- `LMXHttpServer` constructor or options shape.
- `Broker1` constructor or `IBrokerOpts` (the existing `noDelay: bool`
  boot option keeps working; the new `setNoDelay` is purely additive).
- `package.json` dependencies — uses only the existing `chalk` + Node
  stdlib + the routine entry helper already in place.

### Applying upstream

```bash
git clone git@github.com:ORESoftware/live-mutex.git
cd live-mutex
git switch feat/sweeper-fencing-acquire-many-http
git switch -c feat/admin-panel-log-tcp-toggles
git apply /path/to/this/repo/remote/submodules/upstream-patches/lmx-node-slice-c-admin-panel.patch
npm install
npm run build
npx ts-node --transpile-only test/admin-log-tcp-toggle-test.ts
npx ts-node --transpile-only test/admin-otel-toggle-test.ts
git add -A
git commit -m 'admin: reloadable log-level + runtime TCP_NODELAY + interactive admin panel'
git push -u origin feat/admin-panel-log-tcp-toggles
gh pr create --base feat/sweeper-fencing-acquire-many-http --title 'admin: runtime log-level + TCP toggles + admin HTML panel'
```

After the upstream PR merges:

```bash
cd remote/submodules/live-mutex
git fetch origin
git switch feat/sweeper-fencing-acquire-many-http
git pull --ff-only
cd ../../..
git add remote/submodules/live-mutex
# remove the patch file in the same commit:
# git rm remote/submodules/upstream-patches/lmx-node-slice-c-admin-panel.patch
git commit -m 'submodules(live-mutex): bump to upstream admin-panel'
```

### `lmx-rs-slice-b-admin-panel.patch`

- Target submodule: `remote/submodules/rust-network-mutex-rs`
- Upstream repo:    `git@github.com:ORESoftware/live-mutex-rs.git`
- Base branch:      `dev` (this is what the broker is pinned at on the
                    `feat/lmx-rs-submodule-relocate-and-docker-image`
                    branch in this cluster repo).
- Slice:            B in the
                    [admin-panel rollout plan](../../tests/lmx-admin-public-smoke.mjs)
                    documented inline at the top of that script.

What it adds upstream:

- A reloadable `tracing-subscriber::EnvFilter` so log-level can be
  reconfigured at runtime without restarting the broker process. Held in a
  `OnceLock<reload::Handle<EnvFilter, Registry>>` and exposed through new
  `set_log_directives` / `current_log_directives` public functions.
- Runtime-mutable `Arc<AtomicBool>` flags for `TCP_NODELAY` and
  `TCP_QUICKACK`. Threaded through the accept loop and the per-connection
  `AfterRead` hook so a flip takes effect immediately for new connections
  and on the very next inbound frame for existing connections.
- New HTTP admin endpoints, all gated by `LMX_ADMIN_TOKEN` (the same
  shared secret already used for `/admin/otel`):
  - `GET /admin/log` — current directives string
  - `POST /admin/log` — body `{"directives":"info,lmx=debug"}`
  - `GET /admin/tcp` — `{tcp_nodelay, tcp_quickack, tcp_quickack_supported}`
  - `POST /admin/tcp` — body `{"tcp_nodelay"?: bool, "tcp_quickack"?: bool}`
- Interactive HTML admin panel rendered from `src/status.rs`:
  - `localStorage`-backed admin-token input
  - OTel Enable / Disable buttons (delegates to the existing
    `/admin/otel`)
  - Log-directives free-text input + Apply button
  - `TCP_NODELAY` and `TCP_QUICKACK` On / Off buttons. The QUICKACK row
    is rendered with `disabled` buttons + a "not supported on this OS"
    pill on non-Linux builds (the kernel option is Linux-only).
  - All controls are vanilla JS; no framework. Fetch calls send
    `x-admin-token` from `localStorage["lmx-admin-token"]`. Endpoints
    also accept `Authorization: Bearer <token>`.
- Test coverage:
  - `src/status.rs` — admin panel renders, QUICKACK buttons disabled
    when unsupported, `render` reads the live atomics for TCP flags.
  - `tests/integration.rs` — auth, GET/POST round-trips, partial-body
    POSTs, malformed-body 400s for both new endpoints.

What it does **not** change:

- Wire protocol — no new request/response variants, no client breakage.
- `ServerConfig` shape — `tcp_nodelay: bool` and `tcp_quickack: bool`
  remain the boot-time inputs; they're wrapped into `Arc<AtomicBool>`
  inside `run()`, so existing callers (including the in-tree binary
  and the cluster integration tests) keep working unchanged.
- Cargo dependencies — uses `tracing-subscriber::reload` (already
  pulled in via `env-filter` feature) and `parking_lot` (already a
  dep). No new crates.

### Applying upstream

```bash
git clone git@github.com:ORESoftware/live-mutex-rs.git
cd live-mutex-rs
git switch dev
git switch -c feat/admin-panel-log-tcp-toggles
git apply /path/to/this/repo/remote/submodules/upstream-patches/lmx-rs-slice-b-admin-panel.patch
cargo test --tests
git add -A
git commit -m 'admin: reloadable env-filter + runtime TCP flags + interactive admin panel'
git push -u origin feat/admin-panel-log-tcp-toggles
gh pr create --base dev --title 'admin: reloadable log filter + runtime TCP toggles + admin HTML panel'
```

After the upstream PR merges:

```bash
cd remote/submodules/rust-network-mutex-rs
git fetch origin
git switch dev
git pull --ff-only
cd ../../..
git add remote/submodules/rust-network-mutex-rs
# remove the patch file in the same commit:
# git rm remote/submodules/upstream-patches/lmx-rs-slice-b-admin-panel.patch
git commit -m 'submodules(rust-network-mutex-rs): bump to upstream admin-panel'
```
