# `dd-constraint-scheduler`

A Rust **Axum** constraint scheduler for **job-shop**, **nurse-rostering**, and
**timetabling** problems over HTTP and NATS. The core is a priority-rule
**serial schedule generation scheme (SGS)**: tasks are ordered by a priority
rule (critical-path tail by default) and placed at the earliest start that
satisfies precedence, release times, and machine/resource capacity (each machine
runs up to `capacity` tasks concurrently, default 1 = a disjunctive machine).

Complements the MIP/LP solvers with a dedicated CP-style scheduler — fast,
feasibility-first, good for makespan minimisation.

## HTTP

- `GET /healthz`, `GET /metrics`
- `POST /schedule` — returns task start/finish times, makespan, machine
  utilisation, tardiness, and critical-path flags.

```bash
curl -s localhost:8131/schedule -H 'content-type: application/json' -d '{
  "machines": [{"id":"m1","capacity":1}],
  "tasks": [
    {"id":"a","duration":3,"machine":"m1"},
    {"id":"b","duration":2,"machine":"m1","predecessors":["a"]}
  ]
}'
```

Priority rules: `critical-path` (default), `lpt`, `spt`, `edd`, `release`.

## NATS (subjects in `remote/libs/nats/subject-defs`)

| Env | Default subject | Meaning |
| --- | --- | --- |
| `SCHEDULER_SCHEDULE_SUBJECT` | `dd.remote.scheduler.schedule.requests` | inbound requests (queue group `dd-constraint-scheduler`) |
| `SCHEDULER_RESULT_SUBJECT` | `dd.remote.scheduler.schedule.results` | published `scheduler.schedule.v1` results |
| `SCHEDULER_EVENT_SUBJECT` | `dd.remote.events` | runtime events |

`PORT` defaults to `8131`. Set `NATS_URL` to enable the request/result lane.

## Limits & hardening

Inflight-concurrency cap (`SCHEDULER_MAX_INFLIGHT`, default 16); HTTP returns `503` when saturated, NATS applies backpressure. Bounded to 1 000 tasks; per-task `duration`/`release` and machine `capacity` are range-checked so the timeline cannot overflow `u64` or zero a utilisation denominator.

## Authentication

Optional and **off by default** (matching the sibling compute services). Set `SCHEDULER_AUTH_SECRET` (or the shared `SERVER_AUTH_SECRET`) to require callers of `/schedule` to present a matching `x-server-auth: <secret>` (or `auth: <secret>`) header; the comparison is constant-time. When the secret is unset the endpoint is open. `/healthz` and `/metrics` are always open (for probes and Prometheus). Rejections return `401` and increment `*_auth_failures_total`. The deployment manifest wires `SCHEDULER_AUTH_SECRET` from the `dd-agent-secrets` secret with `optional: true`, so enabling auth is a one-key secret edit.
