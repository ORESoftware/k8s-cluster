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

Protected HTTP routes require `SERVER_AUTH_SECRET` through `X-Server-Auth`,
`X-Container-Pool-Auth`, or `X-Agent-Auth`. The gateway injects `X-Server-Auth` for
`/container-pools`.

## Postgres contract

The generic app config table shape lives at `remote/databases/pg/tables/app-config-table.sql`.
Seed the default runtime pools with `remote/databases/pg/seeds/container-pool-app-config.sql`.
The service reads:

- `CONTAINER_POOL_APP_CONFIG_SCOPE`, default `default`
- `CONTAINER_POOL_APP_CONFIG_KEY`, default `container-pool.runtime-pools.v1`

The dedicated fallback table shape lives at
`remote/databases/pg/tables/container-pool-configs-table.sql`. Pool entries in either source use:

- `slug`: stable pool selector used by HTTP and NATS requests.
- `image`: trusted local or registry image for warm containers.
- `command`: optional JSON array appended after the image in `nerdctl run`.
- `env`: JSON object injected as container environment variables.
- `request_path`: default HTTP path inside the worker, usually `/invoke`.
- `container_port`: container listener port when not using host networking.
- `min_warm` and `max_warm`: reconciliation floor and ceiling.
- `request_timeout_ms`: per-dispatch timeout.
- `nats_subject`: optional per-pool subject for NATS request routing.

The service also accepts `CONTAINER_POOL_CONFIG_JSON` as a development fallback. Production should
use Postgres so pool definitions can be changed without a pod rollout.

## Runtime images

Default multi-stage runtime base images live in `runtime-images/` for `nodejs`, `rust`, `golang`,
`python3`, `dart`, `gleamlang`, and `erlang`. Build them on the EC2 node into the local containerd
store before enabling the seed config:

```sh
cd /home/ec2-user/codes/dd/dd-next-1/remote/container-pool-rs
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

This service is intentionally a container-pool control plane, not a shell execution API. It never
accepts arbitrary commands from dispatch requests; process shape comes from trusted Postgres config.
