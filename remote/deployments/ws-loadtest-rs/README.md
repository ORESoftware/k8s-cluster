# `remote/deployments/ws-loadtest-rs`

Rust websocket load generator for remote Gleam server stress testing.

Default behavior:

- opens `5,000` concurrent websocket clients
- targets `ws://dd-gleamlang-server.default.svc.cluster.local:8081/ws`
- keeps each client connected for `300s`, then reconnects
- logs rolling counters every 10 seconds

## Environment variables

- `TARGET_WS_URL` (default above)
- `CLIENT_COUNT` (default `5000`)
- `HOLD_SECONDS` (default `300`)
- `CONNECT_TIMEOUT_SECONDS` (default `20`)
- `RECEIVE_TIMEOUT_SECONDS` (default `5`)
- `RECONNECT_DELAY_MS` (default `1000`)
- `RAMP_DELAY_MS` (default `1`)
- `REPORT_INTERVAL_SECONDS` (default `10`)
- `LOAD_MODE` (default `hold`) — set to `pipeline` to switch from the
  legacy "connect + ping once + hold open" capacity test to a rate-driven
  pipeline test that sends shaped JSON frames and measures end-to-end
  round-trip latency. Designed for driving
  [`dd-akka-ws-server`](../akka-ws-server/readme.md)'s `/ws/asyncjava`
  and `/ws/akkastreams` endpoints for the side-by-side comparison.
- `LOADTEST_TRANSPORTS` (default `http,tcp,websocket`) — advertised
  transport matrix for deployment/runbook automation. This binary's active
  generator is WebSocket; HTTP/TCP coverage for the suite comes from the lock
  load testers and trigger APIs.

### Pipeline mode (`LOAD_MODE=pipeline`) extras

- `MESSAGES_PER_SECOND_PER_CLIENT` (default `10.0`) — per-client send
  rate. With `CLIENT_COUNT=50` and this at `10`, offered load is 500
  msg/s.
- `MESSAGE_PAYLOAD` (default `"a benchmark message body"`) — string
  inserted into the payload field. The logical message shape is
  `{id: "c{client}-{seq}", payload: "{payload}"}`.
- `MESSAGE_ENCODINGS` (default `json`; aliases: `MESSAGE_ENCODING` for
  a single value) — comma-separated encoding list to round-robin over.
  Supported values are `json`, `msgpack`, `protobuf`, and `flatbuffers`.
  JSON is sent as a text WebSocket frame; the other encodings are sent as
  binary frames using the same two-field message model. Protobuf uses field
  `1 = id`, `2 = payload`; FlatBuffers uses a table with `id` then
  `payload`.
- `CORRELATION_TIMEOUT_SECONDS` (default `10`) — pending-request entries
  older than this are swept from the in-memory map so a slow server
  can't OOM the loadtest. Responses that arrive after the sweep are
  counted as `correlation_misses` rather than as round-trip samples.

The pipeline report line replaces `messages` with the latency histogram
(`p50_us / p95_us / p99_us / max_us`) plus
`sent / received / in_flight / correlation_misses / receive_errors`.

`LOAD_MODE=gcs` always sends the chat.vibe JSON envelope expected by the
GCS WebSocket service. The encoding list is still logged on startup so the
GCS deployments share the same operator-visible matrix, but binary encoding
experiments should use `LOAD_MODE=pipeline` against an endpoint that can echo
or decode those frames.

## Container pool smoke mode

Set `CONTAINER_POOL_URL` to switch from WebSocket load generation to a single container-pool smoke
request. This mode posts one UUID-like `echoKey` to the selected pool and exits after verifying the
container response echoed it back.

- `CONTAINER_POOL_URL` (example: `http://dd-container-pool.default.svc.cluster.local:8102`)
- `CONTAINER_POOL_ROUTE_PREFIX` (default `/pools`; use `/container-pools` through the gateway)
- `CONTAINER_POOL_POOL` (default `rust`)
- `CONTAINER_POOL_AUTH_SECRET` (optional, sent as `X-Server-Auth`)
- `CONTAINER_POOL_ECHO_KEY` (optional; generated when omitted)
- `CONTAINER_POOL_TIMEOUT_SECONDS` (default `30`)

## Build and run

```bash
docker build -t dd-ws-loadtest-rs:dev remote/deployments/ws-loadtest-rs
docker run --rm dd-ws-loadtest-rs:dev
```
