# `remote/deployments/gleamlang-server`

This directory owns the Kubernetes service name `dd-gleamlang-server`, which is the live gateway
target for `/gleam/*`. The EC2 manifest runs the unified
[`../gleamlang-ws-server`](../gleamlang-ws-server) implementation under that stable service name so
the live `/gleam/ws` path gets the Postgres-backed presence store, sharded LISTEN/NOTIFY, and
explicit WAL opt-in while preserving the existing gateway, secret, and service wiring.

Small Gleam + OTP websocket service with a supervised runtime:

- child 1: `broadcaster` actor (ticks every 2s and emits JSON payloads)
- child 2: `mist` HTTP/websocket server

The Kubernetes deployments also run a `nats-bridge` sidecar. The sidecar uses the singleton
`nats-client.mjs` connection to read NATS task events (`dd.remote.events`) and POST them over
localhost TCP to Gleam `POST /broadcast`. Gleam broadcasts the payload to connected browser
websockets. The same sidecar exposes a localhost-only `POST /publish` endpoint so WebSocket client
messages can be written back to NATS on `dd.remote.websocket.events`. The bridge and HTTP server
both require `GLEAM_BROADCAST_SECRET`, sourced from `dd-gleamlang-server-secrets`, before internal
fanout or publish endpoints accept events.

The server exposes:

- `GET /` -> redirect to `/home`
- `GET /home` -> lightweight controls page that opens a websocket
- `GET /healthz` -> JSON health check
- `GET /metrics` -> Prometheus metrics for active WebSocket connections, ticks, HTTP requests,
  NATS-bridged task events, and WebSocket client messages
- `GET /ws` -> websocket stream (`{"type":"tick","sequence":...}`)
- `POST /broadcast` -> internal localhost fanout endpoint for the NATS bridge
- sidecar `POST http://127.0.0.1:8083/publish` -> internal localhost NATS publish endpoint

## Project layout

```
remote/deployments/gleamlang-server/
├── gleam.toml
├── Dockerfile
├── nats-bridge.mjs
├── nats-client.mjs
└── src/
    ├── gleamlang_server.gleam
    └── gleamlang_server/
        ├── broadcaster.gleam
        └── http_server.gleam
```

## Local run

```bash
cd remote/deployments/gleamlang-server
gleam run
```

Default bind is `0.0.0.0:8081`.

## Docker

```bash
docker build -t dd-gleamlang-server:latest remote/deployments/gleamlang-server
docker run --rm -p 8081:8081 dd-gleamlang-server:latest
```

Then open `http://localhost:8081/home`.

## Kubernetes Target

- `k8s/ec2`: uses the EC2 host checkout at `/home/ec2-user/codes/dd/dd-next-1` and runs
  `remote/deployments/gleamlang-ws-server` from source inside the `dd-gleamlang-server` pod.

```bash
kubectl apply -k remote/deployments/gleamlang-server/k8s/ec2
```

## Argo CD applications

GitOps app manifests:

- `remote/argocd/apps/dd-gleamlang-server.application.yaml` (EC2 path)
