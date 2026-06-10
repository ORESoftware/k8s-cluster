# `dd-sat-smt-server`

A small Rust **Axum** service that solves **CNF SAT** problems and lightweight
**SMT-style** constraint encodings over HTTP and NATS. The core is an iterative
**DPLL** with unit propagation, pure-literal elimination, and a conflict budget
so pathological instances return `unknown` instead of hanging. On top of raw CNF
it accepts cardinality sugar (`atMostOne` / `atLeastOne` / `exactlyOne`) compiled
down to clauses — handy for scheduling, configuration, and graph-colouring.

Pairs with the formal-methods servers: throw a constraint problem at it and read
the model back.

## HTTP

- `GET /healthz` — liveness.
- `GET /metrics` — Prometheus counters.
- `POST /solve` — solve a problem; returns `sat` / `unsat` / `unknown` plus a
  model and DPLL stats.

```bash
curl -s localhost:8130/solve -H 'content-type: application/json' -d '{
  "variables": ["a", "b"],
  "clauses": [[{"var":"a"},{"var":"b"}], [{"var":"a","negated":true}]],
  "exactlyOne": [[{"var":"a"},{"var":"b"}]]
}'
```

## NATS (source-of-truth subjects in `remote/libs/nats/subject-defs`)

| Env | Default subject | Meaning |
| --- | --- | --- |
| `SAT_SOLVE_SUBJECT` | `dd.remote.sat.solve.requests` | inbound solve requests (queue group `dd-sat-smt-server`) |
| `SAT_RESULT_SUBJECT` | `dd.remote.sat.solve.results` | published `sat.solve.v1` results |
| `SAT_EVENT_SUBJECT` | `dd.remote.events` | runtime lifecycle/telemetry events |

Set `NATS_URL` to enable the request/result lane; without it the service is
HTTP-only. `PORT` defaults to `8130`.

## Limits & hardening

Requests are bounded by an inflight-concurrency cap (`SAT_MAX_INFLIGHT`, default 16): HTTP returns `503` when saturated and the NATS loop applies backpressure. Instances are capped at 2 000 variables / 100 000 clauses; DPLL enforces a per-solve work ceiling (returns `unknown` rather than spinning) and runs on a dedicated large-stack thread so deep search cannot overflow the worker stack. `conflictBudget` is request-tunable.
