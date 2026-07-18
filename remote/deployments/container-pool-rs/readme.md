# `dd-container-pool`

Rust service for keeping containerd-backed workers warm and dispatching requests to them.

The service reads active pool definitions from Postgres `app_config` key
`container-pool.runtime-pools.v1`, falls back to table `container_pool_configs`, reconciles the
desired number of warm containers with `nerdctl`, and exposes both HTTP and NATS dispatch surfaces:

- `GET /healthz` reports service readiness and whether Postgres/NATS are configured.
- `GET /metrics` exposes Prometheus text metrics.
- `GET /pools` and `GET /pools/:pool` show configured pools and warm container counts.
- `POST /pools/:pool/warm` reconciles a pool immediately.
- `POST /pools/:pool/dispatch` leases an idle warm container, posts JSON to it, and releases it.
- NATS requests on `CONTAINER_POOL_NATS_SUBJECT` use the same dispatch path. A message may include
  `poolSlug` or `poolId`; if omitted, the service can infer the pool from the configured
  `natsSubject`.

The reconciler keeps at least `min_warm` available request slots, not merely `min_warm` running
processes. If a singleton pool has one warm worker and that worker is leased, the service starts a
replacement in the background up to `max_warm`. Idle surplus containers are removed after
`idle_ttl_seconds`.

When a dispatch includes `affinityKey`, the manager can acquire a Redis lock for
`poolId:affinityKey` before selecting, starting, or posting to the worker. The EC2 deployment wires
this to `dd-redis-cache` so concurrent task requests for the same thread wait for the first
container match/startup to finish instead of racing duplicate `nerdctl run` calls. Redis locking is
enabled by `CONTAINER_POOL_REDIS_URL`; when unset, local development keeps the previous in-process
behavior. Lock ownership is checked with `WATCH`/`MULTI`/`EXEC` on release because Redis scripts are
disabled in the shared cache ACL.

Remote-dev task dispatches also set `freshAffinity: true`. For a new `affinityKey`, that prevents
the pool from binding the thread to an unbound container that has already handled a request; the
thread can use an already-bound same-key worker, a never-used warm worker, or a newly started
container. Follow-up tasks for the same thread keep using the same affinity-bound worker.

`nerdctl -n k8s.io run -d` for a cold worker uses
`CONTAINER_POOL_NERDCTL_RUN_TIMEOUT_SECONDS` (default 180s) so a busy containerd host can finish
namespace, cgroup, and overlay setup without timing out. Short bookkeeping commands such as
`inspect` and `rm` stay on the smaller `CONTAINER_POOL_COMMAND_TIMEOUT_SECONDS` budget.

Warm workers are health checked on the configured `health_path` (default `/healthz`). The manager
also verifies the container is still running through `nerdctl inspect`; failed health checks mark the
container unhealthy, retire it after the configured threshold, and reconcile the pool back to its
available-capacity floor.

## Operator UI: `/container-pool/config`

The web UI at `/container-pool/config` (gateway-gated, same operator cookie as
`/lambdas/functions`) lists every pool image in the catalog, exposes the on-disk Dockerfile as the
sane default, lets an operator save edits as new revisions, and runs an isolated `nerdctl build` +
smoke-test against any revision. Saved revisions and build/test runs are stored in
`container_pool_image_revisions` and `container_pool_build_runs` (see
`remote/libs/pg-defs/schema/schema.sql`). Builds run inside the `dd-pool` containerd namespace
under a candidate tag like `<image>-cpool-test:<sha>` so they never collide with the live
production tag — promoting a revision into production is a separate, manual step (rebuild the real
tag via `idle-reaper-rs` or `nerdctl build` on the host once a candidate passes review).

The build/test flow is wired through `dd-remote-rest-api`, which reuses the same
`/run/containerd/containerd.sock` + `/usr/local/bin/nerdctl` hostPath mounts as the existing lambda
image builds. The API is gateway-gated and also requires `X-Server-Auth` by default
(`CONTAINER_POOL_IMAGE_API_AUTH_REQUIRED=true`) so direct in-cluster callers cannot trigger builds
without the service secret. Enable the surface with `CONTAINER_POOL_IMAGE_BUILDS_ENABLED=true`;
tune limits with `CONTAINER_POOL_IMAGE_BUILD_TIMEOUT_SECONDS` (default 1200) and
`CONTAINER_POOL_IMAGE_TEST_TIMEOUT_SECONDS` (default 120). Custom smoke commands are disabled by
default (`CONTAINER_POOL_IMAGE_CUSTOM_TEST_COMMANDS_ENABLED=false`); the catalog default command is
used unless an operator deliberately enables that escape hatch.

Protected HTTP routes require `SERVER_AUTH_SECRET` through `X-Server-Auth`,
`X-Container-Pool-Auth`, or `X-Agent-Auth`. The gateway injects `X-Server-Auth` for
`/container-pools`.

## Postgres contract

The generic app config table shape is the `app_config` block in
`remote/libs/pg-defs/schema/schema.sql` (the single source of truth for every shared
table). Seed the default runtime pools with
`remote/databases/pg/seeds/container-pool-app-config.sql`. The service reads:

- `CONTAINER_POOL_APP_CONFIG_SCOPE`, default `default`
- `CONTAINER_POOL_APP_CONFIG_KEY`, default `container-pool.runtime-pools.v1`

The dedicated fallback table shape is the `container_pool_configs` block in
`remote/libs/pg-defs/schema/schema.sql`. Pool entries in either source use:

- `slug`: stable pool selector used by HTTP and NATS requests.
- `image`: trusted local or registry image for warm containers.
- `command`: optional JSON array appended after the image in `nerdctl run`.
- `env`: JSON object injected as container environment variables.
- `readOnly`: optional bool; defaults to true for generic runtimes. Repo-scoped
  chat/Claude workers set this false because they need a writable checkout.
- `user`: optional container user. Generic runtimes use `10001:10001`; the
  Node chat/Claude worker image uses `1000:1000`.
- `request_path`: default HTTP path inside the worker, usually `/invoke`.
- `health_path`: worker health endpoint, usually `/healthz`.
- `container_port`: container listener port when not using host networking.
- `min_warm` and `max_warm`: reconciliation floor and ceiling.
- `request_timeout_ms`: per-dispatch timeout.
- `nats_subject`: optional per-pool subject for NATS request routing.
- `mounts` (alias `volumes`): optional array of `{source|volume, target|mountPath, readOnly?}`
  entries mounted into every warm container of the pool. See "Shared-volume code/binaries" below.
- `unconfined`: optional bool (default false). Opts a mounted-code pool out of the automatic
  `--cap-drop ALL`/`--security-opt no-new-privileges` hardening (see below). It does **not** grant
  `--privileged` or add capabilities — it only falls back to the service-level security flags.

The service also accepts `CONTAINER_POOL_CONFIG_JSON` as a development fallback. Production should
use Postgres so pool definitions can be changed without a pod rollout.

## Shared-volume code/binaries (generic runtimes)

Rather than baking a separate image per language/function, a pool can run a **generic runtime
image** and pull the *code or compiled binary* from a shared volume at start (zero-copy). The image
supplies the runtime/libc; the mount supplies the code; `command` and `env` are the per-pool flags.
This scales to many (20-30+) runtimes/functions without rebuilding images for code changes — only the
volume contents change. The warm container still runs a long-lived server (listening on `$PORT`,
serving `health_path`/`request_path`); `command` should start that server from the mounted path, e.g.

```json
{
  "slug": "rust-svc-foo",
  "image": "docker.io/library/dd-container-pool-rust-runtime:dev",
  "mounts": [{ "source": "dd-code", "target": "/opt/code", "readOnly": true }],
  "command": ["/opt/code/bin/foo-server"],
  "env": { "FOO_FLAG": "1" }
}
```

`source` is either a nerdctl/docker **named volume** (always permitted) or an **absolute host path**.
Host paths are rejected unless they sit under a prefix in `CONTAINER_POOL_MOUNT_SOURCE_ALLOWLIST`
(comma-separated, matched on a path boundary). Mounts are **read-only by default**; a `readOnly:false`
mount additionally requires `CONTAINER_POOL_ALLOW_WRITABLE_MOUNTS=true`. `target` must be an absolute
in-container path (no `..`); `:` and `,` are disallowed in both so the `-v src:dst:mode` arg is
unambiguous. Up to 16 mounts per pool. Policy violations fail the container start with a clear error
(visible in reconcile logs) rather than silently dropping the mount. Mounts are surfaced per pool in
`GET /pools`. The `command`/`mounts` shape still comes only from trusted Postgres config — never from
dispatch requests — so this stays a control-plane capability, not a shell-exec API.

Because a mounted-code pool runs code the image did not bake in, such pools are **confined by
default**: the manager applies `--cap-drop ALL` and `--security-opt no-new-privileges` to them even
when the service-level `CONTAINER_POOL_CAP_DROP_ALL` / `CONTAINER_POOL_NO_NEW_PRIVILEGES` are off
(those default off and govern mount-less pools). A normal server running as uid `10001` on the
injected high `$PORT` needs no capabilities, so this is transparent; set `unconfined: true` on the
pool only for the rare binary that genuinely needs one (it then falls back to the service flags and
still does not get `--privileged`). On SELinux-enforcing hosts, host-path bind mounts may need a
relabel (`:z`/`:Z`) to be readable — prefer named volumes there, since `:Z` relabels the host path
and can disrupt other consumers.

## Runtime images

Default multi-stage runtime base images live in `runtime-images/` for `nodejs`, `rust`, `golang`,
`python3`, `dart`, `gleamlang`, and `erlang`. Build them on the EC2 node into the local containerd
store before enabling the seed config:

```sh
cd /home/ec2-user/codes/dd/dd-next-1/remote/deployments/container-pool-rs
nerdctl -n k8s.io build -f runtime-images/nodejs.Dockerfile -t docker.io/library/dd-container-pool-nodejs-runtime:dev .
nerdctl -n k8s.io build -f runtime-images/rust.Dockerfile -t docker.io/library/dd-container-pool-rust-runtime:dev .
nerdctl -n k8s.io build -f runtime-images/golang.Dockerfile -t docker.io/library/dd-container-pool-golang-runtime:dev .
nerdctl -n k8s.io build -f runtime-images/python3.Dockerfile -t docker.io/library/dd-container-pool-python3-runtime:dev .
nerdctl -n k8s.io build -f runtime-images/dart.Dockerfile -t docker.io/library/dd-container-pool-dart-runtime:dev .
nerdctl -n k8s.io build -f runtime-images/gleamlang.Dockerfile -t docker.io/library/dd-container-pool-gleamlang-runtime:dev .
nerdctl -n k8s.io build -f runtime-images/erlang.Dockerfile -t docker.io/library/dd-container-pool-erlang-runtime:dev .
```

## Runtime model

The Kubernetes deployment runs privileged, with host networking and the EC2 containerd socket
mounted. Warm containers are launched with labels:

- `dd.container-pool.managed=true`
- `dd.container-pool.pool=<slug>`
- `dd.container-pool.service=dd-container-pool`

By default the manager calls `/usr/local/bin/nerdctl -n k8s.io run -d`, allocates ports from
`CONTAINER_POOL_PORT_START..CONTAINER_POOL_PORT_END`, and posts to `127.0.0.1:<allocatedPort>`.
`CONTAINER_POOL_NETWORK=host` is the default for the EC2 runtime; workers should listen on the
injected `PORT` value.

### Container engines and OCI runtimes

The engine is selected with `CONTAINER_POOL_ENGINE` (default `nerdctl`; also `docker`, `podman`) —
these share one Docker-UX `run`/`ps`/`inspect`/`rm` flag surface. nerdctl scopes to the containerd
namespace via the global `-n` (`CONTAINER_POOL_CONTAINERD_NAMESPACE`); docker/podman take no
namespace. The binary path is `CONTAINER_POOL_ENGINE_BIN` (falls back to the legacy
`CONTAINER_POOL_NERDCTL_BIN`, else a per-engine default).

The low-level **OCI runtime** is orthogonal and set with `CONTAINER_POOL_OCI_RUNTIME`, passed through
as `--runtime` under whichever engine is active. This covers `runc` (default when unset), `crun`,
and sandboxed runtimes — gVisor (`runsc`, or `io.containerd.runsc.v1` under containerd/nerdctl) and
Kata Containers (`io.containerd.kata.v2`). The value is validated to a runtime handler name or an
absolute binary path (no whitespace/shell metacharacters); a set-but-invalid value is **ignored with
a startup warning** rather than silently dropping to the engine default — important because for a
sandbox runtime that silent fallback (e.g. intending gVisor/Kata, getting `runc`) would be an
isolation downgrade. So `nerdctl` over `containerd` with
`--runtime io.containerd.kata.v2` gives Kata-isolated warm pools; `docker --runtime runsc` gives
gVisor. Any OCI image (built declaratively from a Dockerfile) works across all of these.

**Out of scope:** `LXD` (system containers, not Dockerfile/OCI images) and `CRI-O` (driven via
`crictl` with a pod-sandbox + container JSON spec, not a `docker run`-style CLI) use fundamentally
different command models and are not driven by this manager. Use one of the Docker-UX engines above
to run OCI images; select `runc`/`crun`/Kata/gVisor via `--runtime` for the isolation profile.

## Worker contract

Every managed worker image should implement this convention:

- Listen on `0.0.0.0:$PORT`; the pool manager allocates `PORT` per warm container.
- Serve `GET $DD_POOL_HEALTH_PATH` and return 2xx when ready. Default: `/healthz`.
- Serve `POST $DD_POOL_REQUEST_PATH` and accept a JSON request envelope. Default: `/invoke`.
- Echo/debug workers should return the submitted `echoKey` or `key` for smoke testing.
- If `NATS_URL` is injected, publish lifecycle events to `DD_POOL_NATS_EVENT_SUBJECT` and
  heartbeats to `DD_POOL_NATS_HEARTBEAT_SUBJECT`.

The bundled runtime images use a common Python HTTP shim that implements `/healthz`, `/invoke`,
optional NATS `started`/`heartbeat`/`request.*` messages, and hands request bodies to the trusted
runtime-specific handler configured by `DD_POOL_HANDLER`.

Repo-scoped Node chat/Claude pools are a separate trusted worker shape. They use
the generic `dd-dev-server:dev` image, keep `min_warm` workers per configured
repo/base branch, accept task dispatches on `/tasks`, and stream task events
through outbound WebSocket plus NATS. The repo URL and base branch are supplied
through pool config/env; they are not hardcoded into the Dockerfile.

This service is intentionally a container-pool control plane, not a shell execution API. It never
accepts arbitrary commands from dispatch requests; process shape comes from trusted Postgres config.

> **ORM policy:** prefer **SeaORM** over sqlx for new database code (MASH stack: maud, axum, SeaORM, supabase, htmx).
