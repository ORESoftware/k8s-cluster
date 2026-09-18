# dd-evolution-optimizer (evolution-rs)

A distributed **island-model genetic algorithm** for continuous black-box minimization.
It slots into the existing solver fleet alongside `mdp-optimizer-rs` and the in-house
MIP solver node, reusing the same NATS JetStream master/worker pattern, and pairs with
`dd-data-viz-rs`'s evolutionary-visualization marks.

## Model

- **master** — exposes HTTP `POST /optimize`, partitions a population across N *islands*,
  runs `epochs` epochs, and **migrates elite individuals around a ring** between epochs
  (island `i` donates its best `migrationSize` individuals to island `i+1`, displacing the
  receiver's worst). It aggregates the global best and a per-epoch fitness history.
- **island** — a JetStream pull-consumer worker. Each epoch it evolves one subpopulation
  for `generationsPerEpoch` generations (tournament selection, BLX-0.25 blend crossover,
  Gaussian mutation, elitism) and publishes the fitness-sorted result back.

The GA core (`src/ga.rs`) is pure and deterministic from a `u64` seed — it uses a
SplitMix64 RNG rather than pulling in `rand`, matching the dependency-light style of the
solver fleet. With **no `NATS_URL`** the master evolves every island locally, so a single
pod is fully functional for development.

## Subjects

Sourced from `dd-nats-subject-defs` (schema: `libs/nats/subject-defs/schema/evolution.schema.json`):

| Constant | Subject |
|---|---|
| `EVOLUTION_JOBS_SUBJECT` | `dd.remote.evolution.jobs` (queue group `dd-evolution-optimizer-islands`) |
| `EVOLUTION_RESULTS_SUBJECT` | `dd.remote.evolution.results` |
| `EVOLUTION_MIGRANTS_SUBJECT` | `dd.remote.evolution.migrants` |
| `EVOLUTION_EVENTS_SUBJECT` | `dd.remote.evolution.events` |

JetStream stream `DD_REMOTE_EVOLUTION` carries all four. Islands scale on consumer lag via
KEDA (`island-scaledobject.yaml`).

## HTTP

```
GET  /            service info, supported functions, limits
GET  /healthz     liveness
GET  /readyz      readiness (role + NATS connectivity)
GET  /metrics     Prometheus text
POST /optimize    run an optimization (master only)
```

### Example

```bash
curl -s localhost:8131/optimize -H 'content-type: application/json' -d '{
  "problem": { "function": "rastrigin", "dimension": 10, "lowerBound": -5.12, "upperBound": 5.12 },
  "islands": 6,
  "populationPerIsland": 80,
  "generationsPerEpoch": 50,
  "epochs": 12,
  "migrationSize": 6,
  "seed": 42
}'
```

Built-in benchmark functions (all global-minimum 0): `sphere`, `rosenbrock`, `rastrigin`,
`ackley`. `sphere` is convex; the rest are multimodal and exercise migration well.

## Build & deploy

```bash
# from the k8s-cluster repo root
docker build -f remote/deployments/evolution-rs/Dockerfile -t dd-evolution-optimizer:dev .
kubectl apply -k remote/deployments/evolution-rs/k8s
```

`cargo test` runs the GA core's determinism/convergence unit tests.
