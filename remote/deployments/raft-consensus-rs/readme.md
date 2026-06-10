# `dd-raft-consensus`

An in-process **Raft consensus simulator** in Rust (**Axum** over HTTP and
NATS). It runs a virtual cluster of N nodes through a discrete-tick
message-passing network, driving leader election and log replication under
configurable **chaos** — message drops, time-boxed network partitions, and node
crash/restart schedules. It is a simulator, not a real distributed node:
deterministic, reproducible, and built to be hammered by the existing loadtest
harnesses. Pairs with `nats` / `rust-network-mutex-rs`.

Each run returns the **committed log**, the term→leader history, election
counts, and an explicit **divergence check** (Raft's State Machine Safety: no two
nodes may commit different commands at the same index). Per-step transitions fan
out on `dd.remote.raft.consensus.events`.

## HTTP

- `GET /healthz`, `GET /metrics`
- `POST /simulate`

```bash
curl -s localhost:8135/simulate -H 'content-type: application/json' -d '{
  "nodes": 5, "ticks": 3000, "commandCount": 20,
  "dropProbability": 0.1,
  "partitions": [{"fromTick":500,"toTick":1200,"groups":[[0,1],[2,3,4]]}],
  "crashes": [{"node":2,"downFrom":1500,"upAt":1800}]
}'
```

`safetyHolds` should stay `true` under any chaos configuration — that is the
property to assert when chaos-testing.

## NATS (subjects in `remote/libs/nats/subject-defs`)

| Env | Default subject | Meaning |
| --- | --- | --- |
| `RAFT_PROPOSE_SUBJECT` | `dd.remote.raft.propose.requests` | inbound scenario requests (queue group `dd-raft-consensus`) |
| `RAFT_RESULT_SUBJECT` | `dd.remote.raft.consensus.results` | published `raft.consensus.v1` results |
| `RAFT_EVENT_SUBJECT` | `dd.remote.raft.consensus.events` | per-run transition summary fan-out |
| `RAFT_RUNTIME_SUBJECT` | `dd.remote.events` | runtime events |

`PORT` defaults to `8135`. Set `NATS_URL` to enable the request/result lane.

## Limits & hardening

Inflight-concurrency cap (`RAFT_MAX_INFLIGHT`, default 16); HTTP returns `503` when saturated, NATS applies backpressure. Simulations are bounded to 9 nodes, 50 000 ticks, and 5 000 commands; partition/crash node ids are validated against the cluster size.
