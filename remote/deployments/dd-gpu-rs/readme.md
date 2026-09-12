# `dd-gpu-rs`

A Rust **Axum** **GPU job scheduler** — *submit work, get result*. It assigns a
batch of AI/ML jobs onto a fleet of GPUs over HTTP and NATS. Each GPU has a VRAM
capacity and may run **several jobs concurrently** as long as the sum of the
resident jobs' VRAM stays within that capacity — the natural model for AI
workloads, where you pack models/inference onto a card while memory fits and
time-share when it does not.

The core is a greedy **list-scheduling** scheme: jobs are ordered by priority
then longest duration (LPT), and each is placed on the GPU + earliest feasible
start that keeps concurrent VRAM within capacity and yields the **earliest
finish** (ties broken toward the card with more free headroom). Complements the
slot-based `dd-constraint-scheduler` with a memory-capacity, concurrent-occupancy
placer built for GPU fleets.

## HTTP

- `GET /healthz`, `GET /metrics`
- `POST /schedule` — returns per-job GPU assignment with start/finish times,
  makespan, per-GPU memory utilisation, wait times, and any rejected jobs.

```bash
curl -s localhost:8136/schedule -H 'content-type: application/json' -d '{
  "gpus": [
    {"id":"a100-0","memoryMib":80000,"tags":["a100","fp16"]},
    {"id":"h100-0","memoryMib":80000,"speed":2.0,"tags":["h100","fp8"]}
  ],
  "jobs": [
    {"id":"train-llm","vramMib":60000,"durationMs":3600000,"priority":10},
    {"id":"infer-batch","vramMib":12000,"durationMs":60000},
    {"id":"quantize","vramMib":8000,"durationMs":30000,"requiresTags":["fp8"]}
  ]
}'
```

- `speed` (per GPU, default `1.0`): throughput multiplier; a job's effective
  duration on that GPU is `ceil(durationMs / speed)`.
- `priority` (per job, default `0`): higher schedules first.
- `gpu` (per job): pin to a specific GPU id (hard affinity).
- `requiresTags` (per job): job runs only on a GPU whose `tags` are a superset.
- `releaseMs` (per job, default `0`): earliest the job may start.

A job whose VRAM exceeds every eligible GPU (or whose affinity/tags match none)
is returned under `rejections` rather than placed; `feasible` is `false` when any
job is rejected.

## NATS (subjects in `remote/libs/nats/subject-defs`)

| Env | Default subject | Meaning |
| --- | --- | --- |
| `GPU_JOB_SUBJECT` | `dd.remote.gpu.jobs.requests` | inbound requests (queue group `dd-gpu-rs`) |
| `GPU_RESULT_SUBJECT` | `dd.remote.gpu.jobs.results` | published `gpu.schedule.v1` results |
| `GPU_EVENT_SUBJECT` | `dd.remote.events` | runtime events |

`PORT` defaults to `8136`. Set `NATS_URL` to enable the request/result lane.

## Limits & hardening

Inflight-concurrency cap (`GPU_MAX_INFLIGHT`, default 16); HTTP returns `503`
when saturated, NATS applies backpressure. Bounded to 2 000 jobs and 256 GPUs;
per-job `vramMib`/`durationMs`/`releaseMs`, GPU `memoryMib`, and GPU `speed` are
range-checked so the timeline and VRAM sums cannot overflow `u64`.
