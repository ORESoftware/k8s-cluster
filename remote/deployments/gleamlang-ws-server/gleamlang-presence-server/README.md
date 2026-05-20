# gleamlang-presence-server

Multi-node WebSocket presence service in Gleam / Erlang/OTP, designed to
run as a `StatefulSet` on Kubernetes. Each pod holds an in-memory routing
table for its own WebSocket connections; cluster-wide membership is
discovered via Erlang's built-in `pg` module; conversation membership is
durable in Postgres with an in-memory cache.

## What it answers

> "How do BEAM nodes find each other in k8s? Is `pg` reliable? Can `pg`
> and ETS work cleanly in tandem?"

Yes, yes, and yes — and this code implements the standard answers.

## Architecture

```
        ┌──── pod presence-0 (BEAM node) ─────────┐
        │                                          │
        │  ┌──────────────┐    ┌───────────────┐  │
ws ────►│  │ mist ws hand │    │ ETS registry  │  │
clients │  │ per conn proc│◄──►│ ByUser / ByConv│  │
        │  └──────┬───────┘    └───────┬───────┘  │
        │         │                    │           │
        │         │ JoinConv/LeaveConv │           │
        │         ▼                    ▼           │
        │  ┌──────────────┐    ┌───────────────┐  │
        │  │ conversations│    │ fanout relay  │  │
        │  │ in-mem cache │    │ (named, per   │  │
        │  │ + PG source  │    │  node, pg)    │  │
        │  └──────┬───────┘    └───────┬───────┘  │
        │         │                    │           │
        │         ▼                    ▼           │
        │       Postgres        Erlang `pg`  ─────┼──► other pods
        │       (durable)       (gossip)           │     (mesh)
        └──────────────────────────────────────────┘
```

- **ETS group registry** (local, microsecond reads, typed `Subject` per
  row). Indexed on four `ConnGroup` axes: `ByUser`, `ByUserDevice`,
  `ByConv`, `ByUserConv`.
- **Erlang `pg`** (cross-node PID membership, replicated, eventually
  consistent).
- **Fanout relay** — one named process per node. Cluster-wide broadcasts
  send one envelope per peer node, not one per remote subscriber.
- **Conversations actor** — in-memory cache backed by Postgres, mesh-
  gossips membership events to peer nodes via `pg` for fast cache
  convergence when not running with a shared store. Emits
  `MembershipChanged(_, AddedToConv(members))` (with the conv's current
  member list inlined, so clients don't need a `GET
  /conv/<id>/members` roundtrip) to `ByUser` on add, and
  `MembershipChanged(_, RemovedFromConv)` to `ByUser` plus
  `Kick("removed from conv …")` to `ByUserConv` on remove.
- **Cluster discovery** — periodic loop that queries the k8s API for
  peer pods (or accepts a static `CLUSTER_PEERS` list for local dev),
  then calls `net_kernel:connect_node/1` for any new ones. The mesh
  self-completes once any two nodes connect.

## Quick start

### Single node
```bash
gleam run                                    # boots on :8081 with in-memory store
curl localhost:8081/healthz
python3 scripts/demo.py                      # runs the e2e demo (28 checks)
```

### Three-node local cluster
```bash
./scripts/cluster-local.sh up                # spawns presence0/1/2 on :8181/2/3
python3 scripts/demo.py \
  --bases http://localhost:8181 http://localhost:8182 http://localhost:8183
./scripts/cluster-local.sh down              # stops them
```

### Load test
```bash
cd ../ws-loadtest-rs && cargo build --release
TARGET_WS_URL="ws://localhost:8181/ws?user=load-test" \
CLIENT_COUNT=500 HOLD_SECONDS=10 LOAD_MODE=hold \
  ./target/release/ws-loadtest-rs
```

## Connection topology

A single device opens MULTIPLE websockets to this server:

- exactly one **user-scoped** ws (`/ws?user=<userId>`) which receives
  membership-change notifications (`added-to <conv>` / `removed-from
  <conv>`) so the client knows when to open or close per-conv
  websockets;
- one **conv-scoped** ws per active conversation
  (`/ws?user=<userId>&conv=<convId>`) which receives that conv's
  broadcast frames.

Both variants accept an optional `&device=<deviceId>` query param so
device-targeted sends (e.g. "log out this device") can address every ws
of one device.

## Routes

| Method | Path                                              | Effect                                                                |
|--------|---------------------------------------------------|-----------------------------------------------------------------------|
| GET    | `/`                                               | Plain-text help.                                                      |
| GET    | `/healthz`                                        | JSON health.                                                          |
| GET    | `/nodes`                                          | Self + connected BEAM peers.                                          |
| GET    | `/ws?user=<id>`                                   | Open a **user-scoped** ws.                                            |
| GET    | `/ws?user=<id>&conv=<convId>`                     | Open a **conv-scoped** ws. 403 if user isn't a member.                |
| GET    | `/ws?...&device=<id>`                             | Optional on either variant; sets the device dimension.                |
| POST   | `/conv/<id>/members/<user>`                       | Add user to conv (durable + cluster broadcast).                       |
| DELETE | `/conv/<id>/members/<user>`                       | Remove user from conv (kicks the user's conv-ws's).                   |
| GET    | `/conv/<id>/members`                              | List members.                                                         |
| POST   | `/conv/<id>/broadcast`                            | Body broadcast to every conv-scoped ws of every member.               |
| POST   | `/user/<id>/broadcast`                            | Body broadcast to every user-scoped ws of `<id>` on every node.       |
| POST   | `/user/<u>/devices/<d>/logout`                    | Close every ws (user- and conv-scoped) of one device of one user.     |

## Wire format

Every server-originated frame is a JSON object with a `type` discriminator:

| `type`              | Sent when                                                          | Extra fields                                                            |
|---------------------|--------------------------------------------------------------------|-------------------------------------------------------------------------|
| `hello`             | Immediately after ws upgrade, on every ws.                         | `scope` (`user`\|`conv`), `user`, `conv`\|null, `device`\|null, `node`. |
| `membership-changed`| User joined/left a conv (delivered on the user-scoped ws).         | `change` (`added`\|`removed`), `conv`, `members` (only on `added`).     |
| `kick`              | Server is closing this ws (e.g. removed from conv, device logout). | `reason`.                                                               |
| `re-registered`     | Registry actor crashed + restarted; we just re-joined.             | (none)                                                                  |

Application payloads sent via `/conv/<id>/broadcast` or `/user/<id>/broadcast` are
forwarded **verbatim**; clients distinguish them from system frames by checking
the first non-whitespace byte (`{` + presence of a `type` field ⇒ system frame).

The `hello` frame lets clients confirm what scope the server interpreted and
which BEAM node accepted the upgrade — useful when sitting behind a k8s `Service`
that hashes upgrades across pods.

## Environment

| Var                              | Default                 | Notes                                                    |
|----------------------------------|-------------------------|----------------------------------------------------------|
| `PORT`                           | `8081`                  | HTTP/WS listen port.                                     |
| `PG_DATABASE_URL`                | (in-memory)             | If set, opens a pog pool; otherwise in-memory fallback.  |
| `CLUSTER_PEERS`                  | (empty)                 | Comma-separated full node names. Wins over k8s mode.     |
| `CLUSTER_NAMESPACE`              | `default`               | k8s namespace for pod discovery.                         |
| `CLUSTER_LABEL_SELECTOR`         | `app=presence`          | k8s label selector.                                      |
| `CLUSTER_NODE_PREFIX`            | `presence`              | Erlang short-name prefix.                                |
| `CLUSTER_HEADLESS_SERVICE`       | `presence-svc`          | Headless Service name for DNS.                           |
| `CLUSTER_DISCOVERY_INTERVAL_MS`  | `5000`                  | Discovery loop interval.                                 |
| `RELEASE_NODE`                   | (k8s sets it)           | Full long-name node, e.g. `presence@presence-0.…`.       |
| `RELEASE_COOKIE`                 | (k8s sets it)           | Shared Erlang cookie. Mount from Secret.                 |
| `ERL_AFLAGS`                     | (pin dist ports)        | Recommended: `-kernel inet_dist_listen_min 9100 inet_dist_listen_max 9100`. |

## Kubernetes

```bash
kubectl apply -f k8s/00-namespace.yaml
kubectl apply -f k8s/10-rbac.yaml
# edit k8s/20-secret-cookie.yaml first — replace placeholder
kubectl apply -f k8s/20-secret-cookie.yaml
kubectl apply -f k8s/30-headless-service.yaml
kubectl apply -f k8s/40-statefulset.yaml
kubectl apply -f k8s/50-network-policy.yaml
```

## Notes on `pg` + ETS

- `pg` itself is internally ETS-backed: `pg:get_local_members/2` is a
  microsecond ETS read, not an RPC.
- We keep a separate ETS table because `pg` only stores `Pid`, not the
  typed Gleam `Subject(ConnMsg)` we need for in-process sends.
- `pg` is eventually consistent; convergence is sub-second on a healthy
  cluster. Net-splits remove a partitioned node's PIDs from each side's
  view; reconnection re-syncs automatically.
- Group keys can be any Erlang term. We use Gleam variants
  (`ByUser(_)` / `ByConv(_)`) which encode as tagged tuples — fine for
  `pg`. Avoid atoms-from-untrusted-strings.
