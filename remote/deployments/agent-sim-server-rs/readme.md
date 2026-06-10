# `dd-agent-sim-server`

A Rust **Axum** agent-based / cellular-automata simulator streamed over NATS for
live demos (in the spirit of the soccer live server). Four models share one
harness:

- `life` — Conway's Game of Life (toroidal Moore neighbourhood)
- `sir` — stochastic SIR epidemic spread on a grid
- `schelling` — Schelling segregation with a tolerance threshold
- `boids` — continuous flocking (alignment / cohesion / separation)

Each run produces a per-step time series plus a bounded set of full frames. The
service fans frames out on `dd.remote.agent_sim.frames` **and** bridges them to
the shared websocket subject so browser clients can animate them, then publishes
the final result. Deterministic via a seeded SplitMix64 PRNG.

## HTTP

- `GET /healthz`, `GET /metrics`
- `POST /simulate` — set `model`, grid `width`/`height` (or `agents` for boids),
  `steps`, and model params. `frameDelayMs` paces the live stream; `includeFrames`
  echoes frames in the HTTP response.

```bash
curl -s localhost:8133/simulate -H 'content-type: application/json' -d '{
  "model": "sir", "width": 64, "height": 64, "steps": 200,
  "infectionRate": 0.3, "recoveryRate": 0.1, "frameStride": 5
}'
```

## NATS (subjects in `remote/libs/nats/subject-defs`)

| Env | Default subject | Meaning |
| --- | --- | --- |
| `AGENT_SIM_SIMULATE_SUBJECT` | `dd.remote.agent_sim.simulate.requests` | inbound requests (queue group `dd-agent-sim-server`) |
| `AGENT_SIM_FRAME_SUBJECT` | `dd.remote.agent_sim.frames` | per-tick frame fan-out |
| `AGENT_SIM_WS_SUBJECT` | `dd.remote.websocket.events` | websocket bridge for browser animation |
| `AGENT_SIM_RESULT_SUBJECT` | `dd.remote.agent_sim.simulate.results` | published `agent_sim.simulate.v1` results |
| `AGENT_SIM_EVENT_SUBJECT` | `dd.remote.events` | runtime events |

`PORT` defaults to `8133`. Set `NATS_URL` to enable streaming.

## Limits & hardening

Inflight-concurrency cap (`AGENT_SIM_MAX_INFLIGHT`, default 8); HTTP returns `503` when saturated, NATS applies backpressure. CPU is bounded by compute budgets — grid `cells × steps ≤ 60M`, boids `agents² × steps ≤ 200M` — independent of the individual size caps. `frameDelayMs` is clamped to 100 ms and total streamed pacing to 15 s, so one request cannot hold a worker for minutes.
