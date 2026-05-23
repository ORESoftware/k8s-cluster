# dd-go-wss-server

Minimal high-throughput Go WebSocket server purpose-built as the **Go**
peer in the cross-language WSS benchmark (`Dart` / `Rust` / `Gleam` /
`Akka` / `Go`).

## Architecture (mirrors `dd-rust-wss-server`)

- Single pod, single Go process.
- Concurrency = goroutines scheduled across `GOMAXPROCS` OS threads.
- WS port (default `8107`) is bound by N independent acceptor
  goroutines, each owning its own `net.Listener` with `SO_REUSEPORT`.
  The kernel hashes incoming SYNs across the pool — same model as
  Dart's gateway-shard isolates that bind 8089 with `shared: true` and
  the Rust acceptor tasks that bind 8097 with `set_reuseport(true)`.
- Admin port (default `8108`) hosts `/metrics`, `/healthz`, `/readyz`
  on a separate `net/http` server so probe + Prometheus traffic can
  never queue behind WS work.

## Wire protocol

Identical to `dd-rust-wss-server` so the same `ws-loadtest-rs`
`LOAD_MODE=pipeline` driver works verbatim.

| Inbound | Reply |
| --- | --- |
| `{"type":"ping","id":"<id>","ts":<u64>}` | `{"type":"pong","id":"<id>","ts":<ms>}` |
| `{"id":"<id>","payload":"..."}` | `{"ok":true,"result":{"id":"<id>"}}` |
| text `"ping"` | `{"type":"pong","ts":<ms>}` |

Anything else is dropped silently (kept off the hot path).

## Metrics

`/metrics` on the admin port exposes:

- `dd_go_ws_active` — currently connected clients (gauge)
- `dd_go_ws_connections_total` — accepted connections (counter)
- `dd_go_ws_closed_total` — closed connections (counter)
- `dd_go_ws_messages_in_total` / `dd_go_ws_messages_out_total`
- `dd_go_ws_handshake_failures_total`
- `dd_go_ws_acceptors{id="<n>"}` — one gauge per acceptor goroutine
- `dd_go_ws_uptime_seconds`

## Environment

| Var | Default | Notes |
| --- | --- | --- |
| `HOST` | `0.0.0.0` | bind host |
| `WS_PORT` | `8107` | WS gateway port |
| `ADMIN_PORT` | `8108` | metrics + probes |
| `WS_GATEWAY_SHARDS` | `8` | acceptor goroutine count |
| `GOMAXPROCS` | (auto) | set by the Go runtime to match cgroup CPU limit on Go 1.22+ |

## Build / run locally

```
go build -o dd-go-wss-server .
./dd-go-wss-server
```

In the cluster the pod runs `go build` on first boot, then `exec`s the
binary; subsequent restarts are warm via the on-host build cache.
