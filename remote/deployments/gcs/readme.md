# GCS - Golang Chat Server

This deploys `ORESoftware/chat.vibe` as `gcs` in the default namespace.

## Source layout

`chat.vibe` is tracked here as a git submodule at
`remote/deployments/gcs/chat-vibe` (see `.gitmodules` at the repo root). The
submodule's own remote stays
`git@github.com:ORESoftware/chat.vibe.git`, so day-to-day work on the
chat server happens in that repo as usual. Bumping the deployed
version is a two-step:

```bash
# 1. inside the submodule
cd remote/deployments/gcs/chat-vibe
git fetch origin dev4
git reset --hard origin/dev4   # or any specific commit

# 2. back in k8s-cluster — record the new pointer
cd -
git add remote/deployments/gcs/chat-vibe
git commit -m "bump chat.vibe -> $(git -C remote/deployments/gcs/chat-vibe rev-parse --short HEAD)"
git push origin dev
```

On the EC2 host (`/home/ec2-user/codes/dd/dd-next-1`):

```bash
git pull --ff-only origin dev
git submodule update --init --recursive --remote
```

The bootstrap script (`remote/ec2/bootstrap-amazon-linux-2023-k8s.sh`)
already does the submodule init on cluster bootstrap; the snippet
above is for an existing host that just needs a refresh.

## Build model

The EC2 deployment mounts the in-repo submodule path via hostPath:

```txt
/home/ec2-user/codes/dd/dd-next-1/remote/deployments/gcs/chat-vibe
```

This matches the established cluster pattern — every other Go / Rust
/ TypeScript service that builds-on-startup (dd-build-server,
dd-formal-methods-service, dd-web-scraper, dd-gleamlang-server, etc.)
mounts from `/home/ec2-user/codes/dd/dd-next-1`.

The pod builds the Go binary into an `emptyDir` at startup. This avoids needing
ECR credentials while the chat service image pipeline is being cleaned up.

The current chat server starts two listeners:

- REST API: `gcs.default.svc.cluster.local:3000`
- WebSocket API: `gcs.default.svc.cluster.local:3001`
- Gateway REST health: `https://54.91.17.58/gcs/health` with the operator `Auth` header or
  `dd_auth` browser cookie
- Gateway WebSocket listener health: `https://54.91.17.58/gcs/ws-health` with the operator `Auth`
  header or `dd_auth` browser cookie

Health checks use:

```txt
/chat/v1/health/3000
```

## Backing Services

The EC2 kustomize app includes small in-cluster backing services:

- `gcs-mongodb` on `27017`, running as a single-node StatefulSet replica set
  (`rs0`) so chat.vibe transaction-backed REST handlers work in the EC2 dev
  cluster. The stable member address is provided by
  `gcs-mongodb-headless`.
- `gcs-rabbitmq` on `5672`, with management on `15672`
- `gcs-kafka` on `9092`, with a single KRaft controller/broker
- existing `dd-redis-cache` on `6379`

RabbitMQ and Kafka have persistent hostPath data as requested:

```txt
/var/lib/dd/gcs/rabbitmq
/var/lib/dd/gcs/kafka
```

MongoDB is also persistent:

```txt
/var/lib/dd/gcs/mongodb
```

`gcs` / chat.vibe does not consume Postgres `LISTEN/NOTIFY` or logical WAL
streams. Its deployed runtime uses MongoDB for chat persistence and
RabbitMQ/Kafka/Redis for brokered fan-out/cache paths; WebSocket pod affinity is
handled by `gcs-router`.

### MongoDB sharding posture

chat.vibe can be sharded, but it should use MongoDB-native sharding rather than
the Postgres LISTEN/NOTIFY or WAL pipeline. The current EC2 deployment is only a
single-node replica set for dev transactions; a production sharded deployment
needs config servers, `mongos`, and multiple shard replica sets before running
`sh.shardCollection`.

Use the same routing dimensions as the websocket router and broker topic names:

| Collection | Primary routing key | Notes |
| --- | --- | --- |
| `vibe_chat_conv_message` | `{ ChatId: "hashed" }` | Conversation history, message sends, and room reads should distribute by conv. |
| `vibe_chat_conv_message_ack` | `{ ChatId: "hashed" }` | Acks are usually read with `ChatId` plus `MessageId`/`UserId`. |
| `vibe_chat_conv_users` | `{ ChatId: "hashed" }` | Keep membership rows for one conversation colocated; retain the `UserId` secondary index for inbox-style lookups. |
| `vibe_user_devices` | `{ UserId: "hashed" }` | Device state is user-scoped. |
| `vibe_chat_conv_events` | `{ ChatId: "hashed" }` | Conversation event stream follows the owning conv. |
| `vibe_chat_user` | keep unsharded initially, or shard by `{ _id: "hashed" }` once unique `Handle` constraints are redesigned | Non-shard-key unique indexes are the main blocker here. |

Before enabling native Mongo sharding, audit unique indexes: MongoDB requires
unique indexes on sharded collections to include the shard key. In particular,
the message collection currently has a unique `UserId + DateCreatedOnDevice +
ChatId` index, so either the shard key or the uniqueness contract has to change
before `shardCollection` will be accepted safely.

## Gateway Paths

The EC2 gateway routes require the operator `Auth` header or the `dd_auth` browser cookie before
they forward to chat.vibe:

- `/gcs/health` -> REST health check
- `/gcs/ws-health` -> WebSocket listener health check
- `/gcs/api/...` -> REST API with `/gcs/api` rewritten to `/chat`
- `/gcs/ws/...` -> WebSocket service (via `gcs-router`)

### WebSocket path scheme

WebSocket connections go through `dd-remote-gateway` -> `gcs-router` ->
`gcs` pods. `gcs-router` decides which pod to land on based on the path:

| Path                              | Pool          | Algorithm                  | Why                                                                                                          |
| --------------------------------- | ------------- | -------------------------- | ------------------------------------------------------------------------------------------------------------ |
| `/gcs/ws/conv/<convId>[/...]`     | `gcs_conv`    | `hash $request_uri consistent` | All WS for the same conv pin to the same gcs pod, so `writeToConvTopic` can fan out in-memory (skip broker). |
| `/gcs/ws/user/<userId>[/...]`     | `gcs_balanced`| `least_conn`               | User-topic conns don't share fan-out with other users — distribute by load.                                  |
| `/gcs/ws/device/<deviceId>[/...]` | `gcs_balanced`| `least_conn`               | Same as user.                                                                                                |
| `/gcs/ws/<anything else>`         | `gcs_balanced`| `least_conn`               | Catch-all. Includes legacy / unstructured paths.                                                             |

The gcs server (`ServeHTTP` on port 3001) accepts WS upgrades on any
path, so server-side it doesn't matter which path the client uses. The
path is purely a routing hint for `gcs-router`.

Response header: `gcs-router` adds `X-Gcs-Pool: conv-hash | least-conn`
on every response so clients / browser devtools can see the routing
decision.

### Scaling `gcs` past 1 pod

`gcs-router` is a small Go reverse proxy (no NGINX) that lives in the
`chat.vibe` submodule at `src/gcs-router/`. It watches
`EndpointSlices` for `gcs-headless` via the in-cluster Kubernetes
API and maintains a deterministic FNV-1a Ketama-style ring with
`--vnodes` virtual nodes per pod IP. Scaling `gcs` from 1 -> N
pods is picked up automatically within a few seconds — no
`kubectl rollout restart deployment/gcs-router` needed.

Routing rules:

- `/conv/<convId>[/...]` -> consistent-hash pool keyed on the convId
  segment. Pinned same-conv WS connections to the same gcs pod so
  in-pod fan-out keeps working.
- everything else -> least-connections pool over the same live peer
  set.

The router surfaces routing decisions on every response via the
`X-Gcs-Pool` (`conv-hash` or `least-conn`) and `X-Gcs-Upstream`
headers, exposes `/healthz` on the WS listener, and exports
Prometheus-style metrics on `:9100/metrics`.

The earlier NGINX OSS implementation was retired because OSS NGINX
only resolves upstream DNS once at startup and picks a single A
record (the `resolve` keyword is NGINX Plus only) — with a headless
Service pointing at multiple pods, NGINX OSS would route everything
to a single, possibly stale, pod IP, making consistent hashing a
no-op. See the deployment yaml header comment in
`k8s/ec2/gcs-router.deployment.yaml` for the full rationale.

Existing WS connections still stay on their current pod through a
ring change (the proxy can't migrate an upgraded WS); only new WS
upgrades use the updated ring. That is intrinsic to the WebSocket
upgrade model, not a router limitation.

### Scaling `gcs-router` itself

`gcs-router` is stateless; once `gcs` is >= 2 replicas, bump
`gcs-router` to >= 2 replicas too. The `gcs-router` Service round-robins
between router pods (no stickiness needed at this layer).

## Deploy

Apply the Argo CD application:

```bash
kubectl apply -f remote/argocd/apps/gcs.application.yaml
```

Then watch the app:

```bash
argocd app get gcs
kubectl get pods -l app=gcs -n default
```

When the chat image pipeline is ready again, update
`remote/deployments/gcs/k8s/ec2/gcs.deployment.yaml` to the desired image tag and remove the
EC2 hostPath source mount.

## Load test / WS fan-out test

A one-shot in-cluster Job is checked in at
`remote/deployments/gcs/k8s/ec2-loadtest/loadtest-job.yaml`. It mounts the same
`chat-vibe` submodule via hostPath that `gcs.deployment.yaml` mounts and runs
the three chat-vibe `test/cli` scripts sequentially against the in-cluster
`gcs-router` Service (bypassing the public gateway / `dd-remote-gateway` auth):

1. `multi-device-test.js` — 3 convs × 4 users × 3 devices fan-out + isolation
2. `cross-conv-test.js`   — 4 convs × 5 clients cross-conv isolation
3. `test-colocation.sh`   — 6 clients on one conv, asserts conv-hash pinning

The Job lives OUTSIDE the Argo CD source path (`.../k8s/ec2`) on purpose, so
Argo CD does not try to manage / prune it.

```bash
# from the EC2 host (or any kubectl with cluster access):
kubectl apply -f remote/deployments/gcs/k8s/ec2-loadtest/loadtest-job.yaml
kubectl logs -n default -f job/gcs-loadtest
# (auto-cleaned 10min after completion via ttlSecondsAfterFinished)

# To re-run, delete the previous Job first (Job names are immutable):
kubectl delete job/gcs-loadtest -n default --ignore-not-found
kubectl apply -f remote/deployments/gcs/k8s/ec2-loadtest/loadtest-job.yaml
```

The Job prints `gcs_router_routed_total`, `gcs_router_endpoints`, and
`gcs_router_active_conns` from `gcs-router:9100/metrics` before and after the
test runs, so the router's view of the gcs endpoint set (e.g. 3 ready pods) and
its routing decisions are captured alongside the per-test pass/fail output.

Knobs (env on the Job container — edit the manifest to tune):

| env                  | default | meaning                          |
| -------------------- | ------- | -------------------------------- |
| `MD_CONVS`           | 3       | multi-device: conversations       |
| `MD_USERS_PER_CONV`  | 4       | multi-device: users per conv      |
| `MD_DEVICES_PER_USER`| 3       | multi-device: devices per user    |
| `MD_MESSAGES`        | 6       | multi-device: messages per sender |
| `CC_CONVS`           | 4       | cross-conv: conversations         |
| `CC_CLIENTS_PER_CONV`| 5       | cross-conv: clients per conv      |
| `CC_MESSAGES`        | 8       | cross-conv: messages per sender   |
| `COLOC_N`            | 6       | colocation: ws-clients on 1 conv  |
| `COLOC_DURATION`     | 8       | colocation: hold seconds          |
