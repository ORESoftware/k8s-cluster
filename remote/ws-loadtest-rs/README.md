# `remote/ws-loadtest-rs`

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
docker build -t dd-ws-loadtest-rs:dev remote/ws-loadtest-rs
docker run --rm dd-ws-loadtest-rs:dev
```
