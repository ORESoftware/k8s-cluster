# `remote/deployments/gleamlang-ws-server`

Unified Gleam + OTP websocket service that merges the two predecessors:

- **`remote/deployments/gleamlang-server`** — the legacy "broadcaster" service: a
  small mist-based ws server with a 2-second tick stream, a Prometheus
  `/metrics` endpoint, a secret-gated `/worker-ws/<secret>` upgrade
  path, an internal `/broadcast` HTTP endpoint, and a Node.js
  `nats-bridge` sidecar that converts NATS task events on
  `dd.remote.events` into local HTTP fan-outs (and writes WebSocket
  client messages back to NATS on `dd.remote.websocket.events`).

- **`remote/deployments/gleamlang-presence-server`** — the new presence cluster:
  a multi-node BEAM service that uses Erlang `pg` for cross-node
  membership, an ETS subject registry per pod, a fanout relay per
  node, a Postgres-backed conversations actor with optional sharded
  LISTEN/NOTIFY + wal2json CDC + outbox tail, plus an optional native
  NATS transport (no sidecar required) and an in-process k8s API
  pod-discovery loop.

The two predecessor packages are left untouched — this folder is the
single binary that should replace them.

## What lives where

```
remote/deployments/gleamlang-ws-server/
├── gleam.toml
├── manifest.toml
├── Dockerfile
├── nats-bridge.mjs                  legacy NATS<->HTTP sidecar
├── nats-client.mjs                  shared NATS client (sidecar)
├── src/
│   ├── gleamlang_ws_server.gleam     application entry point
│   ├── gleamlang_ws_server_ffi.erl   merged Erlang FFI
│   ├── dd_nats.erl                   minimal in-process NATS gen_server
│   └── gleamlang_ws_server/
│       ├── broadcaster.gleam         legacy 2s tick + dedup actor
│       ├── pg_contract.gleam         dd_pg_defs re-exports
│       ├── http_server.gleam         mist HTTP + websocket routes
│       ├── connection.gleam          per-ws connection process
│       ├── conversations.gleam       PG-backed membership actor
│       ├── store.gleam               pog pool / in-memory store
│       ├── registry.gleam            ETS subject registry
│       ├── groups.gleam              ConnGroup variants
│       ├── fanout.gleam              per-node fanout relay
│       ├── nats_transport.gleam      native NATS pub/sub (uses dd_nats.erl)
│       ├── pg_groups.gleam           pg scope + helpers
│       ├── pg_listen.gleam           sharded LISTEN/NOTIFY
│       ├── pg_outbox.gleam           durable outbox tail
│       ├── pg_wal.gleam              wal2json CDC
│       ├── cluster.gleam             k8s API pod discovery
│       └── wire.gleam                client-facing JSON wire format
├── test/                            gleeunit tests
├── scripts/                         local cluster + demo helpers
└── k8s/                             k8s manifests (ec2 / minikube / cluster)
```

## Routes

The merged service serves both subsystems on the same listener (default
port `8081`):

### Presence subsystem

| Method | Path                                               | Effect                                                            |
|--------|----------------------------------------------------|-------------------------------------------------------------------|
| GET    | `/`                                                | Plain-text help.                                                  |
| GET    | `/healthz`                                         | JSON health.                                                      |
| GET    | `/nodes`                                           | Self + connected BEAM peers.                                      |
| GET    | `/ws?user=<id>`                                    | Open a **user-scoped** ws.                                        |
| GET    | `/ws?user=<id>&conv=<convId>`                      | Open a **conv-scoped** ws. 403 if user isn't a member.            |
| GET    | `/ws?...&device=<id>`                              | Optional on either variant; sets the device dimension.            |
| POST   | `/conv/<id>/members/<user>`                        | Add user to conv (durable + cluster broadcast).                   |
| DELETE | `/conv/<id>/members/<user>`                        | Remove user from conv (kicks the user's conv-ws's).               |
| GET    | `/conv/<id>/members`                               | List members.                                                     |
| POST   | `/conv/<id>/broadcast`                             | Body broadcast to every conv-scoped ws of every member.           |
| POST   | `/user/<id>/broadcast`                             | Body broadcast to every user-scoped ws of `<id>` on every node.   |
| POST   | `/user/<u>/devices/<d>/logout`                     | Close every ws (user- and conv-scoped) of one device of one user. |

### Legacy broadcaster subsystem

| Method | Path                          | Effect                                                                 |
|--------|-------------------------------|------------------------------------------------------------------------|
| GET    | `/home`                       | Lightweight HTML page that opens a worker ws.                          |
| GET    | `/metrics`                    | Prometheus metrics for the broadcaster (subscribers, ticks, http, ws). |
| GET    | `/worker-ws/<secret>`         | Worker ws on the broadcaster tick stream; can also publish frames.     |
| POST   | `/broadcast`                  | Internal localhost fanout endpoint for the NATS bridge sidecar.        |
| sidecar `POST 127.0.0.1:8083/publish` | NATS publish endpoint exposed by the bridge.                |

> Migration note: the legacy `/ws` (broadcaster ticker) is gone — the
> presence subsystem owns `/ws`. Internal/worker clients should switch
> to `/worker-ws/<secret>`. End-user browser clients should use
> `/ws?user=<id>` (presence).

## Local run

```bash
cd remote/deployments/gleamlang-ws-server
gleam run                                    # boots on :8081 with in-memory store
curl localhost:8081/healthz
python3 scripts/demo.py                      # runs the e2e presence demo
```

A three-node local cluster (presence subsystem):

```bash
./scripts/cluster-local.sh up                # spawns ws-server-0/1/2 on :8181/2/3
python3 scripts/demo.py \
  --bases http://localhost:8181 http://localhost:8182 http://localhost:8183
./scripts/cluster-local.sh down              # stops them
```

## Environment

Same union of vars as the two predecessors; everything is optional
unless noted.

| Var                              | Default                                     | Notes                                                            |
|----------------------------------|---------------------------------------------|------------------------------------------------------------------|
| `PORT`                           | `8081`                                      | HTTP/WS listen port.                                             |
| `GLEAM_BROADCAST_SECRET`         | (required for legacy routes)                | Gates `/broadcast`, `/worker-ws/<secret>`. Sidecar shares this.  |
| `GLEAM_WORKER_WS_SECRET`         | falls back to `GLEAM_BROADCAST_SECRET`      | Per-route override for `/worker-ws/<secret>`.                    |
| `GLEAM_NATS_PUBLISH_URL`         | `http://127.0.0.1:8083/publish`             | Sidecar publish endpoint (legacy).                               |
| `NATS_PUBLISH_SUBJECT`           | `dd.remote.websocket.events`                | Default subject for sidecar-mediated publishes.                  |
| `PG_DATABASE_URL`                | (in-memory)                                 | If set, opens a pog pool; otherwise in-memory fallback.          |
| `NATS_URL`                       | (disabled)                                  | If set, native NATS transport boots in-process.                  |
| `PRESENCE_NOTIFY_SHARDS`         | `256`                                       | LISTEN/NOTIFY + WAL shard count.                                 |
| `PRESENCE_OUTBOX_TICK_MS`        | `5000`                                      | Outbox poll interval.                                            |
| `PRESENCE_WAL_TICK_MS`           | `1000`                                      | WAL poll interval.                                               |
| `CLUSTER_PEERS`                  | (empty)                                     | Comma-separated full node names. Wins over k8s mode.             |
| `CLUSTER_NAMESPACE`              | `default`                                   | k8s namespace for pod discovery.                                 |
| `CLUSTER_LABEL_SELECTOR`         | `app=presence`                              | k8s label selector.                                              |
| `CLUSTER_NODE_PREFIX`            | `presence`                                  | Erlang short-name prefix.                                        |
| `CLUSTER_HEADLESS_SERVICE`       | `presence-svc`                              | Headless Service name for DNS.                                   |
| `CLUSTER_DISCOVERY_INTERVAL_MS`  | `5000`                                      | Discovery loop interval.                                         |
| `RELEASE_NODE`                   | (k8s sets it for the cluster flavor)        | Full long-name node, e.g. `presence@presence-0.…`.               |
| `RELEASE_COOKIE`                 | (k8s sets it via Secret for cluster flavor) | Shared Erlang cookie.                                            |
| `ERL_AFLAGS`                     | (cluster flavor pins dist ports)            | Recommended: `-kernel inet_dist_listen_min 9100 inet_dist_listen_max 9100`. |

## Deployment flavors (`k8s/`)

- `k8s/ec2/` — single-replica Deployment with the Node.js `nats-bridge`
  sidecar, builds `gleam run` from a hostpath checkout. Drop-in
  replacement for the legacy `dd-gleamlang-server` ec2 deployment.
- `k8s/minikube/` — single-replica Deployment using a locally-built
  image (`dd-gleamlang-ws-server:dev`). Drop-in replacement for the
  legacy minikube target.
- `k8s/cluster/` — full presence StatefulSet with RBAC + headless
  Service + NetworkPolicy. Replaces the standalone presence stack.

## Notes on combining the two services

The merge is mechanical:

- Gleam module trees are disjoint (`gleamlang_server/*` vs
  `gleamlang_presence_server/*`), so all 13 presence modules and the
  3 legacy modules coexist under `gleamlang_ws_server/`.
- Erlang FFI surfaces are disjoint apart from `getenv/1` ↔ `env/1`,
  which are aliased in `gleamlang_ws_server_ffi.erl`.
- Two `http_server.gleam`s become one — presence is the base, the
  four legacy routes (`/home`, `/metrics`, `/broadcast`,
  `/worker-ws/<secret>`) are grafted on, and the broadcaster Subject
  is threaded through `Deps`.
- Manifest deps are a strict superset (presence adds `gleam_httpc`,
  `gleam_json`, `pog`/`pgo`/`pg_types`, `gleam_time`, `gleam_crypto`,
  `opentelemetry_api`, `backoff`).
