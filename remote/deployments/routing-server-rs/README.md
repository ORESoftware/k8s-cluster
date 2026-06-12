# dd-routing-server (routing-server-rs)

A distributed **vehicle-routing / TSP** solver with a **live canvas dashboard**. It
reuses the solver fleet's master/worker JetStream dispatch (same shape as the in-house
MIP solver node) and animates the incumbent tour as workers race independent restarts.

## Model

- **master** — serves the dashboard at `/` and `POST /api/solve`. It fans out `restarts`
  independent multi-start jobs over NATS JetStream, folds each returned tour into a live
  **incumbent** (best routes so far), and emits a `RoutingEvents` message every time the
  incumbent improves. The dashboard polls `GET /api/solve/{id}` and redraws.
- **worker** — a JetStream pull-consumer. Each job is one randomized construction
  (nearest-neighbour for TSP; angular **sweep** assignment for VRP) refined by
  **2-opt** local search. Workers scale on consumer lag via KEDA.

The core (`src/tsp.rs`) is pure and deterministic from a `u64` seed — SplitMix64 RNG, no
`rand` dependency. With **no `NATS_URL`** the master runs every restart locally, so a
single pod still solves and animates end to end.

- **TSP** (`vehicleCount <= 1`, no depot): one cyclic tour over all stops.
- **VRP** (`vehicleCount > 1`): depot included once per vehicle route; the dashboard draws
  each vehicle's route in its own colour and the depot as a black square.

## Subjects

Sourced from `dd-nats-subject-defs` (schema: `libs/nats/subject-defs/schema/routing.schema.json`):

| Constant | Subject |
|---|---|
| `ROUTING_JOBS_SUBJECT` | `dd.remote.routing.jobs` (queue group `dd-routing-server-workers`) |
| `ROUTING_RESULTS_SUBJECT` | `dd.remote.routing.results` |
| `ROUTING_EVENTS_SUBJECT` | `dd.remote.routing.events` |

JetStream stream `DD_REMOTE_ROUTING` carries all three.

## HTTP

```
GET  /                 live canvas dashboard
GET  /healthz          liveness
GET  /readyz           readiness (role + NATS connectivity)
GET  /metrics          Prometheus text
POST /api/solve        start a solve (master only) -> { solveId }
GET  /api/solve/{id}   current incumbent + progress (polled by the dashboard)
```

### Example

```bash
# Generate a random 150-stop, 5-vehicle instance and solve it with 32 restarts.
curl -s localhost:8132/api/solve -H 'content-type: application/json' -d '{
  "generate": { "count": 150, "vehicles": 5, "seed": 42 },
  "restarts": 32
}'
# -> { "ok": true, "solveId": "route-..." }

curl -s localhost:8132/api/solve/route-...   # poll for the incumbent
```

You can also POST explicit `stops` (`[{ "id", "x", "y" }]`) with `depotIndex` and
`vehicleCount`. Open `http://localhost:8132/` to watch tours improve in real time.

## Hardening

- **Opt-in auth**: set `ROUTING_AUTH_SECRET` and `POST /api/solve` requires it as
  `Authorization: Bearer <secret>` (or `X-DD-Auth`), compared in constant time. Dashboard
  reads (`/`, `/api/solve/{id}`) stay open. The master manifest wires it from
  `dd-agent-secrets` (`optional: true`).
- **Concurrency cap**: at most `MAX_CONCURRENT_SOLVES` (16) background solves run at once;
  each holds a permit for its lifetime and excess requests get `503`.
- **Total time budget**: `timeoutMs` bounds both the distributed collection and the local
  fallback loop, so a big solve can't peg a CPU indefinitely in the background.
- **Untrusted jobs**: workers reject oversize payloads and re-`validate()` the problem
  (stop count, finite coords, depot range) before solving, so a job published straight to
  the subject can't pin a worker; `MAX_STOPS`/`MAX_VEHICLES`/`MAX_LOCAL_PASSES` are clamped.
- **NATS resilience**: bounded connect-retry (`ROUTING_NATS_CONNECT_ATTEMPTS`), and the
  master ensures the JetStream stream exists on startup.
- Tracked solves are capped (`MAX_TRACKED_SOLVES`, oldest evicted); rejections are counted
  in `dd_routing_rejected_requests_total`.

## Build & deploy

```bash
# from the k8s-cluster repo root
docker build -f remote/deployments/routing-server-rs/Dockerfile -t dd-routing-server:dev .
kubectl apply -k remote/deployments/routing-server-rs/k8s
```

`cargo test` runs the TSP/VRP core's determinism + 2-opt convergence unit tests.
