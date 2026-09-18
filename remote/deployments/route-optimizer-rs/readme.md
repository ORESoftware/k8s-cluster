# `dd-route-optimizer`

A Rust **Axum** service for **TSP** and capacitated **VRP with time windows**
over HTTP and NATS.

- **TSP** — nearest-neighbour construction from the depot + **2-opt** local
  search on the closed tour.
- **VRP** — sequential greedy insertion: each vehicle takes the nearest
  time-window-and-capacity-feasible customer (waiting until its ready time when
  early), then returns to the depot. Unservable stops are reported.

Distances are Euclidean over `(x, y)` by default, or supply an explicit
`distanceMatrix`. Complements `mdp-optimizer` with a concrete combinatorial
routing demo.

## HTTP

- `GET /healthz`, `GET /metrics`
- `POST /optimize` — `mode` is `tsp` or `vrp`.

```bash
curl -s localhost:8132/optimize -H 'content-type: application/json' -d '{
  "mode": "vrp",
  "depot": {"x":0,"y":0},
  "vehicles": 2,
  "vehicleCapacity": 10,
  "stops": [
    {"id":"a","x":1,"y":0,"demand":6,"ready":0,"due":50,"service":1},
    {"id":"b","x":2,"y":1,"demand":6}
  ]
}'
```

## NATS (subjects in `remote/libs/nats/subject-defs`)

| Env | Default subject | Meaning |
| --- | --- | --- |
| `ROUTE_OPTIMIZE_SUBJECT` | `dd.remote.route.optimize.requests` | inbound requests (queue group `dd-route-optimizer`) |
| `ROUTE_RESULT_SUBJECT` | `dd.remote.route.optimize.results` | published `route.optimize.v1` results |
| `ROUTE_EVENT_SUBJECT` | `dd.remote.events` | runtime events |

`PORT` defaults to `8132`. Set `NATS_URL` to enable the request/result lane.

## Limits & hardening

Inflight-concurrency cap (`ROUTE_MAX_INFLIGHT`, default 16); HTTP returns `503` when saturated, NATS applies backpressure. Bounded to 1 000 stops; the O(n²) 2-opt pass is skipped above 600 stops (the nearest-neighbour tour is still returned, with a warning).

## Authentication

Optional and **off by default** (matching the sibling compute services). Set `ROUTE_AUTH_SECRET` (or the shared `SERVER_AUTH_SECRET`) to require callers of `/optimize` to present a matching `x-server-auth: <secret>` (or `auth: <secret>`) header; the comparison is constant-time. When the secret is unset the endpoint is open. `/healthz` and `/metrics` are always open (for probes and Prometheus). Rejections return `401` and increment `*_auth_failures_total`. The deployment manifest wires `ROUTE_AUTH_SECRET` from the `dd-agent-secrets` secret with `optional: true`, so enabling auth is a one-key secret edit.
