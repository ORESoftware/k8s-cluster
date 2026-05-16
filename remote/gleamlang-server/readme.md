# `remote/gleamlang-server`

Small Gleam + OTP websocket service with a supervised runtime:

- child 1: `broadcaster` actor (ticks every 2s and emits JSON payloads)
- child 2: `mist` HTTP/websocket server

On EC2, the Kubernetes deployment also runs a `nats-bridge` sidecar. The sidecar keeps a TCP
connection to NATS (`dd.remote.events`), then POSTs each task event over localhost TCP to Gleam
`POST /broadcast`. Gleam broadcasts the payload to connected browser websockets. The bridge and
HTTP server both require `GLEAM_BROADCAST_SECRET`, sourced from `dd-gleamlang-server-secrets`,
before the internal broadcast endpoint accepts events.

The server exposes:

- `GET /` -> redirect to `/home`
- `GET /home` -> lightweight controls page that opens a websocket
- `GET /healthz` -> JSON health check
- `GET /metrics` -> Prometheus metrics for active WebSocket connections, ticks, HTTP requests,
  NATS-bridged task events, and WebSocket client messages
- `GET /ws` -> websocket stream (`{"type":"tick","sequence":...}`)
- `POST /broadcast` -> internal localhost fanout endpoint for the NATS bridge

## Project layout

```
remote/gleamlang-server/
├── gleam.toml
├── Dockerfile
├── nats-bridge.mjs
└── src/
    ├── gleamlang_server.gleam
    └── gleamlang_server/
        ├── broadcaster.gleam
        └── http_server.gleam
```

## Local run

```bash
cd remote/gleamlang-server
gleam run
```

Default bind is `0.0.0.0:8081`.

## Docker

```bash
docker build -t dd-gleamlang-server:latest remote/gleamlang-server
docker run --rm -p 8081:8081 dd-gleamlang-server:latest
```

Then open `http://localhost:8081/home`.

## Kubernetes targets

Two k8s variants are included:

- `k8s/ec2`: uses the EC2 host checkout at `/home/ec2-user/codes/dd/dd-next-1` and runs `gleam run`
  from source inside the pod.
- `k8s/minikube`: uses a local image (`dd-gleamlang-server:dev`) for Minikube.

Build the Minikube image:

```bash
minikube image build -t dd-gleamlang-server:dev remote/gleamlang-server
```

Then apply with:

```bash
kubectl apply -k remote/gleamlang-server/k8s/minikube
```

For EC2:

```bash
kubectl apply -k remote/gleamlang-server/k8s/ec2
```

## Argo CD applications

GitOps app manifests:

- `remote/argocd/apps/dd-gleamlang-server.application.yaml` (EC2 path)
- `remote/argocd/apps/dd-gleamlang-server-minikube.application.yaml` (Minikube path)
