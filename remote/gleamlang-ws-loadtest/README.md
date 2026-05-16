# `remote/gleamlang-ws-loadtest`

Gleam-targeted websocket load generator (JavaScript runtime) for remote Gleam
server stress testing.

Default behavior:

- opens `5,000` concurrent websocket clients
- targets `ws://dd-gleamlang-server.default.svc.cluster.local:8081/ws`
- keeps each client connected for `300s`, then reconnects
- logs rolling counters every 10 seconds

## Environment variables

- `TARGET_WS_URL` (default above)
- `CLIENT_COUNT` (default `5000`)
- `HOLD_SECONDS` (default `300`)
- `CONNECT_TIMEOUT_MS` (default `20000`)
- `RECONNECT_DELAY_MS` (default `1000`)
- `RAMP_DELAY_MS` (default `1`)
- `REPORT_INTERVAL_SECONDS` (default `10`)

## Build and run

```bash
docker build -t dd-gleamlang-ws-loadtest:dev remote/gleamlang-ws-loadtest
docker run --rm dd-gleamlang-ws-loadtest:dev
```
