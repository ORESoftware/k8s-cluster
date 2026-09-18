# dd-rust-wss-server

A minimal high-throughput Rust WebSocket server purpose-built as a benchmark
peer for `dd-dart-server` and `dd-gleamlang-server`. The intent is **parity with
how the Dart pod is structured** so a head-to-head comparison is meaningful.

## Architecture

| Concern | Dart pod (`dd-dart-server`) | This Rust pod |
|---|---|---|
| Process model | 1 pod, ~110 isolates inside (1 coordinator + 1 HTTP + N gateway shards + child host pool) | 1 pod, single `tokio` multi-thread runtime |
| WS port | `8089`, every gateway-shard isolate `bind(shared: true)` (kernel SO_REUSEPORT) | `8097`, every acceptor task owns its own `TcpListener` with `SO_REUSEPORT` |
| Acceptor count | `WS_GATEWAY_SHARDS` (default 8) | `WS_GATEWAY_SHARDS` (default 8) |
| Admin port | `8088` — `/metrics`, `/healthz`, `/readyz`, `/dart/admin/*` | `8098` — `/metrics`, `/healthz`, `/readyz` |
| Per-connection overhead | one session-runtime object inside a pooled host isolate | one `tokio::spawn` task |

The acceptor model is a 1:1 mirror of Dart's `HttpServer.bind(..., shared: true)`
pattern: N independent listeners on the same port, kernel-side load
distribution via SO_REUSEPORT, no userspace coordination.

## Wire protocol

Inbound JSON:

```json
{"type":"ping","id":"<id>","ts":<u64>}    // ping/pong correlation
{"id":"<id>","payload":"..."}             // akka-style envelope (pipeline loader compat)
```

Server replies:

```json
// ping reply
{"type":"pong","id":"<id>","ts":<server_ms>}

// akka-style reply (matches `ws-loadtest-rs` LOAD_MODE=pipeline expectations)
{"ok":true,"result":{"id":"<id>"},"ts":<server_ms>}
```

Plain text `"ping"` → `{"type":"pong","ts":<server_ms>}`. Anything else is
dropped silently.

## Metrics (Prometheus, port 8098)

| Metric | Type | Notes |
|---|---|---|
| `dd_rust_ws_connections_total` | counter | Successful WS handshakes |
| `dd_rust_ws_disconnections_total` | counter | Closed connections |
| `dd_rust_ws_handshake_failed_total` | counter | TCP accepted but WS upgrade failed |
| `dd_rust_ws_active` | gauge | Live connections (analogous to `dart_sessions_live`) |
| `dd_rust_ws_shards_live` | gauge | Acceptor tasks bound on `WS_PORT` |
| `dd_rust_ws_messages_total{direction,kind}` | counter | Per-frame counters |

## Environment

| Var | Default | Meaning |
|---|---|---|
| `HOST` | `0.0.0.0` | Bind address for both ports |
| `WS_PORT` | `8097` | WS listener (multi-acceptor with SO_REUSEPORT) |
| `ADMIN_PORT` | `8098` | Admin axum router |
| `WS_GATEWAY_SHARDS` | `8` | Number of acceptor tasks bound on `WS_PORT` |

## Loadtest

`ws-loadtest-rs` already speaks both protocol shapes (`LOAD_MODE=pipeline` emits
`{id, payload}` and correlates on the `id` field of any reply). A k8s loader
manifest under `remote/deployments/ws-loadtest-rs/k8s/rust-wss/` targets this
service directly.

```bash
# inside the cluster
kubectl exec deploy/dd-ws-loadtest-rs-rust-wss -- printenv | grep TARGET_WS_URL
# -> ws://dd-rust-wss-server.default.svc.cluster.local:8097/
```
