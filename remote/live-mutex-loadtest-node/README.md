# `remote/live-mutex-loadtest-node`

Node.js lock load generator for cluster-local `dd-live-mutex` and `dd-redis-cache`.

Default behavior:

- starts `3` separate Node.js worker processes
- opens `12` live-mutex clients per worker
- targets `dd-live-mutex.default.svc.cluster.local:6970`
- runs `1,000` aggregate lock/acquire/release cycles per second
- spreads traffic over `5` distinct lock keys
- keeps each lock for `0ms` before releasing it
- logs aggregate throughput and latency every 10 seconds

The lock cycle follows the upstream live-mutex README examples: create a `Client`, call
`ensure()`, `acquire(key)`, and then `release(key, { id })`.

For Redis, the same scheduler and key set use `SET key token NX PX <ttl>` to acquire and a Lua
compare-and-delete script to release only the matching token.

## Environment variables

- `LOCK_BACKEND` (default `live-mutex`; set `redis` for Redis locking)
- `BROKER_HOST` (default `dd-live-mutex.default.svc.cluster.local`)
- `BROKER_PORT` (default `6970`)
- `REDIS_HOST` (default `dd-redis-cache.default.svc.cluster.local`)
- `REDIS_PORT` (default `6379`)
- `REDIS_DATABASE` (default `0`)
- `REDIS_PASSWORD` (default empty)
- `REDIS_LOCK_PREFIX` (default `dd-locktest`)
- `REDIS_LOCK_RETRY_DELAY_MS` (default `1`)
- `REQUESTS_PER_SECOND` (default `1000`, aggregate across workers)
- `WORKER_PROCESSES` (default `3`, minimum `3`)
- `CLIENTS_PER_WORKER` (default `12`)
- `LOCK_KEYS` (default `lmx-loadtest-a,lmx-loadtest-b,lmx-loadtest-c,lmx-loadtest-d,lmx-loadtest-e`)
- `LOCK_HOLD_MS` (default `0`)
- `LOCK_TTL_MS` (default `4000`)
- `LOCK_REQUEST_TIMEOUT_MS` (default `3000`)
- `UNLOCK_REQUEST_TIMEOUT_MS` (default `3000`)
- `LOCK_MAX_RETRIES` (default `0`)
- `MAX_IN_FLIGHT_PER_WORKER` (default `2000`)
- `REPORT_INTERVAL_SECONDS` (default `10`)
- `TEST_DURATION_SECONDS` (default `0`, run until stopped)

## Run locally

```bash
npm ci --ignore-scripts
BROKER_HOST=127.0.0.1 npm start
```

Run the sequential comparison harness:

```bash
COMPARE_DURATION_SECONDS=60 npm run compare
```

## Build

```bash
docker build -t dd-live-mutex-loadtest-node:dev remote/live-mutex-loadtest-node
docker run --rm \
  -e BROKER_HOST=host.docker.internal \
  dd-live-mutex-loadtest-node:dev
```
