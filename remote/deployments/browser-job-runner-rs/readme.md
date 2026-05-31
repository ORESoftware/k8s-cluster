# dd-browser-job-runner

A per-job browser scraping spawner. Unlike `dd-browser-test-server` and `dd-selenium-server` (which
run scenarios in-process in a long-lived pod), this service launches **one ephemeral container per
job**: each `POST /run` spawns a fresh `dd-browser-job-worker` container that runs a single
Playwright or Puppeteer scenario, publishes its JSON result to NATS, and exits.

This directory holds two things:

- The Rust (axum) **spawner** at the crate root (`Cargo.toml`, `src/main.rs`).
- The Node/TS **worker** image under `worker/` (`dd-browser-job-worker`).

## Why a spawner

A long-lived browser server keeps Chromium resident and shares state across requests. For untrusted
or bursty scraping we instead want hard isolation and a hard lifetime per job. The spawner mirrors
the repo's existing nerdctl pattern (`dd-container-pool`, `dd-gleam-lambda-runner`): a privileged,
host-network pod drives the node's containerd over `nerdctl`, launching labelled, resource-capped,
`--rm` worker containers and reaping any that overrun.

## Spawner API (port 8106)

All routes are mirrored under `/browser-jobs/...` for the gateway.

| Method/Path | Description |
| --- | --- |
| `POST /run` | Validate a job, spawn one worker container, return `{ jobId, resultSubject, ... }` immediately (HTTP 202). Auth required. |
| `GET /status` | In-flight count, limits, namespace, image, counters. |
| `GET /jobs` | Currently tracked job containers with deadlines and subjects. |
| `GET /tools` | Engines (`playwright`, `puppeteer`) and the worker image. |
| `GET /healthz`, `GET /readyz` | Probes (unauthenticated). |
| `GET /metrics` | Prometheus text (`browser_job_*`). |

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
  "containerName": "dd-browser-job-18f3a2c0d1e0001",
  "deadlineMs": 1717000000000,
  "resultSubject": "dd.remote.browser_jobs.18f3a2c0d1e0001.result",
  "eventsSubject": "dd.remote.browser_jobs.18f3a2c0d1e0001.events",
  "resultFanoutSubject": "dd.remote.browser_jobs.results"
}
```

**Results are published to NATS, not returned over HTTP.** Subscribe to the per-job
`resultSubject` (or the shared `resultFanoutSubject`) to receive the full `RunResult` JSON.

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

Three independent layers:

1. **Worker watchdog** — the worker process hard-exits at `BROWSER_JOB_MAX_MS`.
2. **Spawner tracker** — a background loop force-removes any container past its deadline and prunes
   finished ones, keeping concurrency and `GET /jobs` accurate.
3. **idle-reaper backstop** — `dd-idle-reaper` runs a `BROWSER_JOB_REAP_*` loop that force-removes any
   `dd.browser-job.managed=true` container in `dd-browser-jobs` that outlives its
   `dd.browser-job.deadline-ms` label plus a grace, covering a dead spawner pod.

## Configuration (env)

| Env | Default | Notes |
| --- | --- | --- |
| `PORT` | `8106` | HTTP listen port |
| `SERVER_AUTH_SECRET` | unset | Required for `POST /run` unless `BROWSER_JOB_ALLOW_UNAUTHENTICATED=true` |
| `BROWSER_JOB_NERDCTL_BIN` | `/usr/local/bin/nerdctl` | |
| `BROWSER_JOB_CONTAINERD_NAMESPACE` | `dd-browser-jobs` | Dedicated namespace so reapers target only our containers |
| `BROWSER_JOB_NETWORK` | `host` | Lets `--network host` workers reach the NATS ClusterIP |
| `BROWSER_JOB_IMAGE` | `docker.io/library/dd-browser-job-worker:dev` | Pulled with `--pull=never` |
| `BROWSER_JOB_MAX_CONCURRENT` | `4` | 429 over the cap |
| `BROWSER_JOB_MAX_LIFETIME_SECONDS` | `540` | Hard ceiling (9 min); clamped to ≤ 540 |
| `BROWSER_JOB_DEFAULT_ENGINE` | `playwright` | |
| `BROWSER_JOB_ALLOW_EVALUATE` | `false` | Arbitrary in-page script execution opt-in |
| `NATS_URL` | `nats://dd-nats.messaging.svc.cluster.local:4222` | Injected into each worker |
| `BROWSER_JOB_NATS_SUBJECT_PREFIX` | `dd.remote.browser_jobs` | Per-job subjects are `<prefix>.<jobId>.result` / `.events` |
| `BROWSER_JOB_NATS_RESULT_SUBJECT` | `dd.remote.browser_jobs.results` | Shared fanout subject |

## Building the worker image

The spawner uses `--pull=never`, so the worker image must exist in the `dd-browser-jobs` containerd
namespace on the node:

```
nerdctl -n dd-browser-jobs build -t docker.io/library/dd-browser-job-worker:dev \
  remote/deployments/browser-job-runner-rs/worker
```

## Local checks

```
# spawner
cargo build --release

# worker
cd worker && pnpm install && pnpm run typecheck
```
