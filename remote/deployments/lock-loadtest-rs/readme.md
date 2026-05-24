# dd-lock-loadtest-rs

Rust mutex-broker load tester. Drives an acquire/release workload over
the live-mutex NDJSON TCP wire protocol and exposes an HTTP trigger
API plus Prometheus metrics. Ships as `dd-lock-loadtest-rs` in
namespace `default`.

## What it tests

The same wire protocol is spoken by three brokers in this cluster:

| Service                       | Source                                   | Improvement set |
|-------------------------------|------------------------------------------|-----------------|
| `dd-rust-network-mutex`       | `remote/deployments/rust-network-mutex-rs` | All (Rust port) |
| `dd-live-mutex-submodule`     | `remote/submodules/live-mutex` (branch `feat/sweeper-fencing-acquire-many-http`) | All (Node port of the same set) |
| `dd-live-mutex`               | upstream `live-mutex@0.2.25` from npm    | None — baseline |

This load tester can target any of the three by passing
`brokerHost` / `brokerPort` in the request body of `POST /runs`. The
default (when the request body is empty) is the Rust broker, set via
the `BROKER_HOST` env var on the deployment.

## HTTP API

| Method | Path           | Purpose |
|--------|----------------|---------|
| `GET`  | `/healthz`     | Liveness — always 200. |
| `GET`  | `/metrics`     | Prometheus exposition (counters + per-broker last-run gauges). |
| `POST` | `/runs`        | Start a run. Returns 202 with `runId`; 409 if a run is already in flight. |
| `GET`  | `/runs/active` | Mid-run snapshot or `null`. |
| `GET`  | `/runs/last`   | Last completed run summary or `null`. |

### Run config (`POST /runs` body — all fields optional)

```json
{
  "brokerHost": "dd-rust-network-mutex.default.svc.cluster.local",
  "brokerPort": 6970,
  "durationSeconds": 60,
  "workers": 16,
  "keys": 32,
  "targetRps": 0,
  "ttlMs": 4000,
  "semaphoreMax": null,
  "useAcquireMany": false
}
```

Field semantics:

- `workers` — concurrent TCP connections (each runs an
  acquire→release loop). Capped at 1024.
- `keys` — keyspace cardinality. `1` = single-key contention storm;
  high values = wide uncontended sweep.
- `targetRps` — broker-wide target rate. `0` (or unset) means
  "as fast as workers can drive". The tester translates this into a
  per-worker think-time.
- `ttlMs` — passed through as the broker's `ttl` field. The broker's
  centralised TTL sweeper kicks in if a worker goes wedged.
- `semaphoreMax` — exercises the per-key semaphore code path. Set
  `1` for classic mutex (default), `>1` for true semaphore. Set `0`
  to negative-test broker validation (broker rejects with a clear
  error; the run will count those as `failedAcquires`).
- `useAcquireMany` — every 16th iteration each worker sends an
  `acquire-many` over a 3-key window. **Note:** there is a known
  queuing bug under contention; leave this `false` unless you're
  intentionally exercising that path.

### Run summary (`GET /runs/last`)

```json
{
  "runId": "...",
  "brokerHost": "...",
  "brokerPort": 6970,
  "startedAtMs": 1779599...,
  "finishedAtMs": 1779599...,
  "durationSeconds": 60,
  "workers": 16,
  "keys": 32,
  "acquired": 5_215_530,
  "released": 5_215_530,
  "failedAcquires": 0,
  "failedReleases": 0,
  "fencingViolations": 0,
  "acquireLatencyUsP50": 88,
  "acquireLatencyUsP95": 145,
  "acquireLatencyUsP99": 200,
  "acquireLatencyUsMax": 10207,
  "actualRps": 86925.4
}
```

`fencingViolations > 0` is a real correctness regression — it means
the broker handed out a fencing token less than or equal to one it
had already issued for the same key. Dashboards should alert on the
metric `lock_loadtest_last_fencing_violations`.

## Running locally

```bash
# Terminal 1: broker
cd remote/deployments/rust-network-mutex-rs
LMX_BIND_HOST=127.0.0.1 LMX_TCP_PORT=16970 LMX_HTTP_PORT=16971 \
  cargo run --release

# Terminal 2: load tester
cd remote/deployments/lock-loadtest-rs
HTTP_BIND=127.0.0.1:8120 BROKER_HOST=127.0.0.1 BROKER_PORT=16970 \
  cargo run --release

# Terminal 3: drive
curl -s -X POST http://127.0.0.1:8120/runs \
  -H 'content-type: application/json' \
  -d '{"durationSeconds": 30, "workers": 16, "keys": 32}'

sleep 32 && curl -s http://127.0.0.1:8120/runs/last | jq
```

A clean local run on a single MacBook returns ~87k acquires/sec at
p99=200µs (loopback). Cross-cluster numbers will be lower; see
`docs/lock-broker-bench-procedure.md` for the documented procedure.

## Limitations

- Single in-flight run at a time. If you `POST /runs` while one is
  active you get HTTP 409.
- `acquire-many` queuing under contention: the underlying client
  library has a known issue here (see TODO at the top of
  `remote/submodules/live-mutex/clients/rust/src/lib.rs`). For now,
  keep `useAcquireMany: false` (the default).
- No per-broker concurrent runs. To benchmark all three brokers in
  the same wall-clock window you need three separate replicas of
  this Deployment.
