# `dd-runtime-config`

Centralised runtime-config control plane.

- **Storage**: Redis (`dd-redis-cache` in-cluster). Every record lives under a
  per-env prefix: `dd:rc:{env}:...` where `{env}` is `stage` or `prod`.
- **Fan-out**: every `RUNTIME_CONFIG_PUSH_INTERVAL_SECONDS` (default 300 = 5 min)
  the service POSTs the current snapshot to every registered subscriber's
  `/internal/update-runtime-config` endpoint. The payload IS the new config —
  subscribers never need to talk to Redis themselves.
- **On demand**: the admin UI at `/admin?env=stage|prod` lets operators add or
  delete entries and trigger an immediate push.
- **Pull path (short-lived consumers, lambdas)**: `GET /snapshot/{env}` returns
  the same snapshot shape. `dd-remote-rest-api` proxies this at
  `/api/runtime-config/snapshot/{env}` so containers can pull at boot without
  needing cluster-internal DNS.
- **Schema**: all payload types live in `remote/libs/interfaces/shared` so the
  Rust service, Node dev-server, Python AI/ML pipeline, and Gleam services
  stay byte-compatible.
- **Hardening**: mutating routes and subscriber apply routes fail closed when
  `RUNTIME_CONFIG_SERVER_SECRET` is missing. Local unauthenticated testing has
  to opt in with `RUNTIME_CONFIG_ALLOW_UNAUTHENTICATED=true`.

## Env vars

| Var                                       | Default                                                 | Purpose                                                      |
| ----------------------------------------- | ------------------------------------------------------- | ------------------------------------------------------------ |
| `RUNTIME_CONFIG_REDIS_URL` / `REDIS_URL`  | `redis://dd-redis-cache.default.svc.cluster.local:6379` | Redis connection URL.                                        |
| `RUNTIME_CONFIG_REDIS_PREFIX`             | `dd:rc`                                                 | Key prefix; combined with env to namespace stage vs prod.    |
| `RUNTIME_CONFIG_PUSH_INTERVAL_SECONDS`    | `300`                                                   | Cron cadence (s).                                            |
| `RUNTIME_CONFIG_PUSH_TIMEOUT_SECONDS`     | `10`                                                    | Per-subscriber push timeout (s).                             |
| `RUNTIME_CONFIG_SERVER_SECRET`            | _unset_                                                 | Required `X-Server-Auth` on mutating + push endpoints; also sent to subscribers. |
| `RUNTIME_CONFIG_ADMIN_SECRET`             | _unset_                                                 | Required `X-Admin-Auth` on HTML form posts. Falls back to `RUNTIME_CONFIG_SERVER_SECRET` if unset. |
| `RUNTIME_CONFIG_ALLOW_UNAUTHENTICATED`    | `false`                                                 | Local-dev escape hatch. When false, mutations return 503 and subscribers return 401/503 if no secret is configured. |
| `RUNTIME_CONFIG_ALLOW_EXTERNAL_SUBSCRIBERS` | `false`                                               | Local/test escape hatch. When false, subscriber `applyUrl` hosts must end in `.svc.cluster.local` (plus localhost for tests) and the path must be `/internal/update-runtime-config`. |
| `HOST` / `PORT`                           | `0.0.0.0` / `8110`                                      | Bind address.                                                |

When both secrets are unset, mutation routes fail closed. For local Redis
experiments, set `RUNTIME_CONFIG_ALLOW_UNAUTHENTICATED=true` explicitly.

## HTTP API

| Method | Path                                | Auth          | Purpose                                                                     |
| ------ | ----------------------------------- | ------------- | --------------------------------------------------------------------------- |
| GET    | `/healthz`                          | none          | Liveness.                                                                   |
| GET    | `/metrics`                          | none          | Prometheus metrics.                                                         |
| GET    | `/snapshot/{env}?scope=...`         | none          | Snapshot pull. The rest-api proxies this for short-lived consumers.         |
| GET    | `/entries/{env}?scope=...`          | none          | Alias for the snapshot view.                                                |
| GET    | `/entries/{env}/{scope}/{key}`      | none          | One entry.                                                                  |
| POST   | `/entries/{env}`                    | `X-Server`    | Upsert. Body = `RuntimeConfigUpsertRequest`. Triggers immediate fan-out.    |
| DELETE | `/entries/{env}/{scope}/{key}`      | `X-Server`    | Delete. Triggers immediate fan-out.                                         |
| POST   | `/push/{env}`                       | `X-Server`    | Force-push current snapshot to every subscriber for the env.                |
| GET    | `/subscribers/{env}`                | none          | List subscribers.                                                           |
| POST   | `/subscribers`                      | `X-Server`    | Register a subscriber. Body = `RuntimeConfigRegisterRequest`.               |
| DELETE | `/subscribers/{env}/{name}`         | `X-Server`    | Remove a subscriber.                                                        |
| GET    | `/admin?env=stage\|prod`            | _gateway_     | HTML admin UI (key/value editor, subscribers table, "push now" button).     |

The admin UI POSTs to `/admin/upsert`, `/admin/push/{env}`,
`/entries/{env}/{scope}/{key}/delete`, and `/subscribers/{env}/{name}/delete`.

## Redis key layout

| Key                                           | Type   | Contents                                                  |
| --------------------------------------------- | ------ | --------------------------------------------------------- |
| `dd:rc:{env}:entry:{scope}:{key}`             | string | JSON-encoded `RuntimeConfigEntry`.                        |
| `dd:rc:{env}:entry-index`                     | set    | `{scope}\x1F{key}` members for fast scan of all entries.  |
| `dd:rc:{env}:generation`                      | int    | Monotonic env snapshot generation. Increments on upsert/delete so stale pushes cannot overwrite newer local state. |
| `dd:rc:{env}:subs:{name}`                     | string | JSON-encoded `RuntimeConfigSubscriber` (with push state). |
| `dd:rc:{env}:subs-index`                      | set    | Subscriber names.                                         |

Switch envs by editing the URL query parameter; stage and prod never share
state. Scope `*` is global: every subscriber receives `*` entries plus entries
whose scope matches its own `RUNTIME_CONFIG_SCOPE`, with service-specific keys
winning when the same key exists in both places.

## Wiring a new subscriber

Every Rust deployment with an HTTP listener is already wired via the shared
`dd-runtime-config-client` crate (`remote/libs/runtime-config-client-rs`).
Mounting takes two lines in `main.rs`:

```rust
let app = Router::new()
    // ...existing routes...
    .with_state(state)
    .merge(dd_runtime_config_client::router());

tokio::spawn(dd_runtime_config_client::register_with_control_plane());
```

…plus `dd-runtime-config-client = { path = "../../libs/runtime-config-client-rs" }`
in the service's `Cargo.toml` and these env vars on the deployment:

```yaml
- { name: RUNTIME_CONFIG_SERVICE_NAME, value: dd-<service> }
- { name: RUNTIME_CONFIG_SCOPE,        value: dd-<service> }
- { name: RUNTIME_CONFIG_ENV,          value: stage }            # or prod
- { name: RUNTIME_CONFIG_REGISTER_URL, value: http://dd-runtime-config.default.svc.cluster.local:8110/subscribers }
- { name: RUNTIME_CONFIG_APPLY_URL,    value: http://dd-<service>.default.svc.cluster.local:<port>/internal/update-runtime-config }
- name: RUNTIME_CONFIG_SERVER_SECRET
  valueFrom:
    secretKeyRef: { name: dd-agent-secrets, key: SERVER_AUTH_SECRET, optional: true }
```

`optional: true` keeps bootstrap tolerant while External Secrets syncs, but the
receiver does not accept pushes without the secret unless
`RUNTIME_CONFIG_ALLOW_UNAUTHENTICATED=true` is set. Registration will retry
until the secret is available and the control plane accepts it.

Node services use `registerRuntimeConfigRoutes()` +
`registerWithControlPlane()` from `dev-server/src/runtime-config.ts` (the
helper is kept self-contained per-service because `dev-server` pins its tsc
`rootDir` to `src/`).

### Subscribers wired today

| Service                     | Apply URL                                                                            | Language |
| --------------------------- | ------------------------------------------------------------------------------------ | -------- |
| `dd-agent-worker-broker`       | `http://dd-agent-worker-broker.default.svc.cluster.local:8098/internal/update-runtime-config`                          | Rust   |
| `dd-ai-ml-pipeline`            | `http://dd-ai-ml-pipeline.ai-ml.svc.cluster.local:8099/internal/update-runtime-config`                                 | Python |
| `dd-bastion`                   | `http://dd-bastion.vpn.svc.cluster.local:8111/internal/update-runtime-config`                                          | Rust   |
| `dd-build-server`              | `http://dd-build-server.default.svc.cluster.local:8100/internal/update-runtime-config`                                 | Rust   |
| `dd-container-pool`            | `http://dd-container-pool.default.svc.cluster.local:8102/internal/update-runtime-config`                               | Rust   |
| `dd-contract-service`          | `http://dd-contract-service.default.svc.cluster.local:8101/internal/update-runtime-config`                             | Rust   |
| `dd-des-simulator`             | `http://dd-des-simulator.default.svc.cluster.local:8099/internal/update-runtime-config`                                | Rust   |
| `dd-dev-server-api`            | `http://dd-dev-server-api.default.svc.cluster.local:8080/internal/update-runtime-config`                               | Node   |
| `dd-formal-methods-server`     | `http://dd-formal-methods-server.default.svc.cluster.local:8110/internal/update-runtime-config`                        | Rust   |
| `dd-formal-methods-service`    | `http://dd-formal-methods-service.default.svc.cluster.local:8111/internal/update-runtime-config`                       | Rust   |
| `dd-gleam-lambda-runner`       | `http://dd-gleam-lambda-runner.default.svc.cluster.local:8083/internal/update-runtime-config`                          | Gleam  |
| `dd-gleam-mcp-server`          | `http://dd-gleam-mcp-server.default.svc.cluster.local:8090/internal/update-runtime-config`                             | Gleam  |
| `dd-gleamlang-presence-server` | `http://<pod>.presence-svc.presence.svc.cluster.local:8081/internal/update-runtime-config` (one register per replica)  | Gleam  |
| `dd-gleamlang-server`          | `http://dd-gleamlang-server.default.svc.cluster.local:8081/internal/update-runtime-config`                             | Gleam  |
| `dd-gleamlang-ws-server`       | `http://dd-gleamlang-ws-server.default.svc.cluster.local:8081/internal/update-runtime-config`                          | Gleam  |
| `dd-mdp-optimizer`             | `http://dd-mdp-optimizer.default.svc.cluster.local:8096/internal/update-runtime-config`                                | Rust   |
| `dd-remote-auth`               | `http://dd-remote-auth.default.svc.cluster.local:8083/internal/update-runtime-config`                                  | Rust   |
| `dd-remote-rest-api`           | `http://dd-remote-rest-api.default.svc.cluster.local:8082/internal/update-runtime-config`                              | Rust   |
| `dd-remote-web-home`           | `http://dd-remote-web-home.default.svc.cluster.local:8080/internal/update-runtime-config`                              | Rust   |
| `dd-rust-vapi-phone`           | `http://dd-rust-vapi-phone.default.svc.cluster.local:8113/internal/update-runtime-config`                              | Rust   |
| `dd-trading-server`            | `http://dd-trading-server.default.svc.cluster.local:8103/internal/update-runtime-config`                               | Rust   |
| `dd-wal-gateway`               | _(deployment yaml pending — receiver crate already merged into the binary)_                                            | Rust   |
| `dd-webrtc-signaling`          | `http://dd-webrtc-signaling.default.svc.cluster.local:8095/internal/update-runtime-config`                             | Rust   |

The generated API docs for each subscriber surface the three
`/internal/runtime-config*` routes; see
`remote/deployments/<service>/generated/api-docs.html`. Python and Gleam
services pick up the same routes via per-language receiver helpers:

* Python — `class RuntimeConfigClient` inlined into
  `remote/deployments/ai-ml-pipeline/src/dd_ai_ml_pipeline.py`. Stdlib only
  (`http.server` + `urllib.request` + `threading`), so no new pip deps.
* Gleam — `remote/libs/runtime-config-client-gleam/` (Gleam wrapper +
  Erlang FFI using `persistent_term` for the snapshot and raw `gen_tcp` for
  the registration POST). Each service adds the path dep to its
  `gleam.toml`, three arms to its route case, and one
  `dd_runtime_config_client.start_registration_loop()` call in `main`.

### Not yet wired

* Pure background workers (`dd-idle-reaper`, `dd-remote-queue-consumer`) —
  no HTTP listener, so nothing to push to. Reach them by republishing the
  same key from a service that *is* a subscriber, or by giving the worker
  its own minimal axum listener if hot-swap is required.

## Build

```bash
# Local — connects to localhost redis by default
cd remote/deployments/runtime-config-rs
cargo run --release

# Image — repo root must be the build context so the shared-interfaces path
# dep is included
docker build -f remote/deployments/runtime-config-rs/Dockerfile -t dd-runtime-config:dev .
```
