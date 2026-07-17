# dd-browser-job-runner

A per-job browser scraping orchestrator. Unlike `dd-browser-test-server` and `dd-selenium-server`
(which run scenarios in-process in a long-lived pod), this service runs **one browser per job** with
hard isolation and a hard lifetime, and always delivers the result over NATS. Each `POST /run`
returns a `jobId` immediately (HTTP 202); the `RunResult` JSON is published to NATS when the job
finishes.

This directory holds two things:

- The Rust (axum) **orchestrator** at the crate root (`Cargo.toml`, `src/main.rs`).
- The Node/TS **worker** image under `worker/` (`dd-browser-job-worker`), which is dual-mode (see below).

## Execution paths: pool-first, nerdctl fallback

A long-lived browser server keeps Chromium resident and shares state across requests. For untrusted
or bursty scraping we instead want a fresh browser per job. We get that two ways:

1. **Primary — `dd-container-pool` warm pool.** A `browser-jobs` pool (defined in
   `remote/databases/pg/seeds/container-pool-app-config.sql`) keeps `dd-browser-job-worker`
   containers warm. For each job the orchestrator does a NATS request/reply to the pool's subject
   (`dd.remote.container_pool.browser-jobs.requests`); the pool leases a warm worker, HTTP-dispatches
   the scenario to its `/run`, and replies with the worker's `RunResult`. The orchestrator republishes
   that to the per-job subject + fanout. **The warm worker self-exits after one job**, so the pool
   retires it and reconciles a fresh replacement — one clean browser per job, but with warm-start
   latency and lifecycle owned by the pool.

2. **Fallback — direct `nerdctl`.** When the pool is down, has no responders, errors, or is saturated
   (no warm container available), the orchestrator spawns a short-lived `dd-browser-job-worker`
   container itself via `nerdctl`, mirroring `dd-gleam-lambda-runner`: a privileged, host-network pod
   driving the node's containerd. The fallback worker runs once (one-shot mode) and publishes its own
   result to NATS.

The caller subscribes to the same NATS subjects either way; the path is transparent.

## Dual-mode worker

The single `dd-browser-job-worker` image picks its mode at startup:

- **serve mode** (default; used by the pool): a tiny HTTP server (`GET /healthz`, `POST /run`). The
  pool keeps it warm and dispatches one scenario; after responding it reports unhealthy and exits.
  Chromium is only launched when a job arrives, so a warm idle worker is just a light Node process.
- **one-shot mode** (used by the nerdctl fallback): if `JOB_SPEC_B64` is set, decode it, run once,
  publish the `RunResult` to NATS, and exit.

## Orchestrator API (port 8106)

All routes are mirrored under `/browser-jobs/...` for the gateway.

| Method/Path | Description |
| --- | --- |
| `POST /run` | Validate a job, return `{ jobId, resultSubject, poolSubject, ... }` immediately (HTTP 202), then run it pool-first / nerdctl-fallback in the background. Auth required. |
| `GET /status` | Pool config, NATS connectivity, in-flight fallback count, limits, counters. |
| `GET /jobs` | Currently tracked **fallback** job containers with deadlines and subjects. |
| `GET /tools` | Engines (`playwright`, `puppeteer`) and the worker image. |
| `GET /healthz`, `GET /readyz` | Probes (unauthenticated). |
| `GET /metrics` | Prometheus text (`browser_job_*`, incl. `browser_job_pool_dispatched_total`, `browser_job_pool_failures_total`, `browser_job_fallback_total`). |

### Request body

```jsonc
{
  "engine": "playwright",            // or "puppeteer" (default: BROWSER_JOB_DEFAULT_ENGINE)
  "url": "https://example.com",      // optional opening navigation
  "steps": [                          // bounded DSL, same as dd-browser-test-server
    { "action": "goto", "url": "https://example.com" },
    { "action": "extractText", "selector": "h1", "name": "heading" },
    { "action": "screenshot", "name": "shot" }
  ],
  "viewport": { "width": 1280, "height": 800 },
  "timeoutMs": 60000
}
```

Step actions: `goto`, `click`, `fill`, `select`, `press`, `waitForSelector`, `waitForUrl`,
`waitForTimeout`, `extractText`, `extractAttribute`, `screenshot`, `evaluate` (the last is disabled
unless `BROWSER_JOB_ALLOW_EVALUATE=true`).

### Response (HTTP 202)

```jsonc
{
  "ok": true,
  "status": "accepted",
  "jobId": "18f3a2c0d1e0001",
  "engine": "playwright",
  "deadlineMs": 1717000000000,
  "resultSubject": "dd.remote.browser_jobs.18f3a2c0d1e0001.result",
  "eventsSubject": "dd.remote.browser_jobs.18f3a2c0d1e0001.events",
  "resultFanoutSubject": "dd.remote.browser_jobs.results",
  "poolSubject": "dd.remote.container_pool.browser-jobs.requests"
}
```

**Results are published to NATS, not returned over HTTP.** Subscribe to the per-job
`resultSubject` (or the shared `resultFanoutSubject`) to receive the full `RunResult` JSON, regardless
of whether the pool or the fallback served the job.

## NATS result envelope

The worker publishes a `RunResult` to both the per-job subject and the shared fanout subject:

```jsonc
{
  "ok": true,
  "jobId": "18f3a2c0d1e0001",
  "engine": "playwright",
  "durationMs": 2143,
  "startedAt": "...", "finishedAt": "...",
  "finalUrl": "https://example.com/", "finalTitle": "Example Domain",
  "steps": [ { "index": 0, "action": "goto", "status": "ok", "durationMs": 812 } ],
  "extracted": { "heading": "Example Domain" },
  "screenshots": [ { "name": "shot", "contentType": "image/jpeg", "base64": "...", "bytes": 12345 } ],
  "consoleEntries": [], "pageErrors": []
}
```

## Lifetime / cleanup (no job lives longer than 9 minutes)

**Pool path:** the warm worker self-exits after one job, and the per-job watchdog
(`BROWSER_JOB_MAX_MS`) plus the pool's per-pool `requestTimeoutMs` (540s) bound a running job.
`dd-container-pool` owns retirement, reconcile, and idle TTL for its `dd-pool` containers.

**Fallback path:** three independent layers, scoped to the `dd-browser-jobs` namespace we own:

1. **Worker watchdog** — the one-shot worker process hard-exits at `BROWSER_JOB_MAX_MS`.
2. **Orchestrator tracker** — a background loop force-removes any fallback container past its deadline
   and prunes finished ones, keeping concurrency and `GET /jobs` accurate.
3. **idle-reaper backstop** — `dd-idle-reaper` runs a `BROWSER_JOB_REAP_*` loop that force-removes any
   `dd.browser-job.managed=true` container in `dd-browser-jobs` that outlives its
   `dd.browser-job.deadline-ms` label plus a grace, covering a dead orchestrator pod.

## Configuration (env)

| Env | Default | Notes |
| --- | --- | --- |
| `PORT` | `8106` | HTTP listen port |
| `SERVER_AUTH_SECRET` | unset | Required for `POST /run` unless `BROWSER_JOB_ALLOW_UNAUTHENTICATED=true` |
| `BROWSER_JOB_POOL_ENABLED` | `true` | Try the `dd-container-pool` warm pool before the nerdctl fallback |
| `BROWSER_JOB_POOL_SLUG` | `browser-jobs` | Pool slug sent in the dispatch request |
| `BROWSER_JOB_POOL_SUBJECT` | `dd.remote.container_pool.browser-jobs.requests` | Pool NATS request subject |
| `BROWSER_JOB_POOL_REQUEST_TIMEOUT_MS` | `max_lifetime*1000 + 30000` | NATS request timeout; ≥ the pool's per-pool `requestTimeoutMs` so a slow-but-working pool isn't double-spawned |
| `BROWSER_JOB_NERDCTL_BIN` | `/usr/local/bin/nerdctl` | Fallback only |
| `BROWSER_JOB_CONTAINERD_NAMESPACE` | `dd-browser-jobs` | Fallback namespace so reapers target only our containers |
| `BROWSER_JOB_NETWORK` | `host` | Lets `--network host` fallback workers reach the NATS ClusterIP |
| `BROWSER_JOB_IMAGE` | `docker.io/library/dd-browser-job-worker:dev` | Fallback image, pulled with `--pull=never` |
| `BROWSER_JOB_MAX_CONCURRENT` | `4` | Cap on concurrent **fallback** containers |
| `BROWSER_JOB_MAX_LIFETIME_SECONDS` | `540` | Hard ceiling (9 min); clamped to ≤ 540 |
| `BROWSER_JOB_DEFAULT_ENGINE` | `playwright` | |
| `BROWSER_JOB_ALLOW_EVALUATE` | `false` | Arbitrary in-page script execution opt-in |
| `NATS_URL` | `nats://dd-nats.messaging.svc.cluster.local:4222` | Pool request/reply + result publishing |
| `BROWSER_JOB_NATS_SUBJECT_PREFIX` | `dd.remote.browser_jobs` | Per-job subjects are `<prefix>.<jobId>.result` / `.events` |
| `BROWSER_JOB_NATS_RESULT_SUBJECT` | `dd.remote.browser_jobs.results` | Shared fanout subject |

## Building the worker image

Both paths use `--pull=never`, so the worker image must exist on the node. The pool builds its
configured `baseImages` (the `browser-jobs` entry points at `worker/`) into the `dd-pool` namespace;
the fallback uses the `dd-browser-jobs` namespace. Build into whichever you exercise, e.g.:

```
# fallback namespace
nerdctl -n dd-browser-jobs build -t docker.io/library/dd-browser-job-worker:dev \
  remote/deployments/browser-job-runner-rs/worker
# pool namespace
nerdctl -n dd-pool build -t docker.io/library/dd-browser-job-worker:dev \
  remote/deployments/browser-job-runner-rs/worker
```

## Local checks

```
# spawner
cargo build --release

# worker
cd worker && pnpm install && pnpm run typecheck
```
