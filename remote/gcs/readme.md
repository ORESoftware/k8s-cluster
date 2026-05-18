# GCS - Golang Chat Server

This deploys `ORESoftware/chat.vibe` as `gcs` in the default namespace.

## Source layout

`chat.vibe` is tracked here as a git submodule at
`remote/gcs/chat-vibe` (see `.gitmodules` at the repo root). The
submodule's own remote stays
`git@github.com:ORESoftware/chat.vibe.git`, so day-to-day work on the
chat server happens in that repo as usual. Bumping the deployed
version is a two-step:

```bash
# 1. inside the submodule
cd remote/gcs/chat-vibe
git fetch origin dev4
git reset --hard origin/dev4   # or any specific commit

# 2. back in k8s-cluster — record the new pointer
cd -
git add remote/gcs/chat-vibe
git commit -m "bump chat.vibe -> $(git -C remote/gcs/chat-vibe rev-parse --short HEAD)"
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
/home/ec2-user/codes/dd/dd-next-1/remote/gcs/chat-vibe
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
- Public REST health: `http://54.91.17.58/gcs/health`
- Public WebSocket listener health: `http://54.91.17.58/gcs/ws-health`

Health checks use:

```txt
/chat/v1/health/3000
```

## Backing Services

The EC2 kustomize app includes small in-cluster backing services:

- `gcs-mongodb` on `27017`
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

## Public Gateway Paths

The EC2 gateway routes:

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

`gcs-router`'s upstreams point at the `gcs-headless` Service, which
returns one A record per ready gcs pod. OSS NGINX only resolves
upstream DNS at startup, so when you scale gcs from 1 -> N pods, do:

```bash
kubectl rollout restart deployment/gcs-router -n default
```

That picks up the new endpoint list. Existing WS connections stay on
their current pod (the proxy can't migrate an upgraded WS); only new
WS upgrades use the updated ring.

Future-proof alternatives (when scale-events get frequent):

1. Replace `gcs-router` with the same nginx image plus a sidecar that
   periodically reloads on endpoint change (poll k8s API,
   `nginx -s reload`).
2. Switch the image to `openresty/openresty` and use
   `balancer_by_lua_block` + `lua-resty-balancer` for true dynamic
   peer updates.
3. Replace `gcs-router` entirely with Envoy / Istio. Envoy's EDS picks
   up endpoint changes from the k8s API in seconds, and the consistent-
   hash policy is a one-line `DestinationRule`.

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
`remote/gcs/k8s/ec2/gcs.deployment.yaml` to the desired image tag and remove the
EC2 hostPath source mount.
