# dd-chaos (chaos-rs)

Cluster **chaos-engineering** loops, sitting next to `idle-reaper-rs` in the maintenance
family. It deliberately perturbs the cluster to surface resilience gaps — and, given the
fleet's cross-runtime benchmarking culture (six WebSocket implementations, etc.), gives a
repeatable way to compare how each service recovers.

## Loops

| Loop | What it does | Guarded by |
|---|---|---|
| **pod-kill** | Lists Running pods in target namespaces and deletes up to `blastRadius` random victims per tick. | kill switch, dry-run, blast radius, protected namespaces, protected label |
| **deployment-jitter** | Removes one replica from an allow-listed Deployment, holds, then restores it. | kill switch, dry-run, target allow-list, min-replicas floor |
| **nats-probe** | A request/reply responder + prober that measures live NATS round-trip latency and jitter. | always safe (read-only) |

Every destructive action is published as an auditable `ChaosExperiments` record plus
`ChaosEvents` lifecycle events. The Kubernetes API is reached with a raw `reqwest` client
and the ServiceAccount bearer token — no `kube-rs` dependency — exactly like `idle-reaper-rs`.

> True network partitioning / packet-level faults need a CNI or mesh fault layer (e.g.
> Chaos Mesh). This service covers pod- and deployment-level faults plus NATS latency
> observation, which is what it can do safely from a single pod.

## Safety (ships OFF)

```
CHAOS_ENABLED=false   # master kill switch — destructive actions are skipped until true
CHAOS_DRY_RUN=true    # even when enabled, only logs/publishes what it WOULD do
```

Destructive actions run only when **`CHAOS_ENABLED=true` AND `CHAOS_DRY_RUN=false`**
(`dd_chaos_armed 1` in metrics). Additional guards:

- `CHAOS_BLAST_RADIUS` (default `1`) — max victims per tick.
- `CHAOS_PROTECTED_NAMESPACES` (default `kube-system,kube-public,kube-node-lease,messaging,default`)
  — never touched, enforced in code regardless of RBAC.
- `CHAOS_PROTECTED_LABEL` (default `dd.dev/chaos-protected`) — pods labelled `…=true` are spared.
  The chaos pod itself carries this label.

## Subjects

Sourced from `dd-nats-subject-defs` (schema: `libs/nats/subject-defs/schema/chaos.schema.json`):

| Constant | Subject |
|---|---|
| `CHAOS_EXPERIMENTS_SUBJECT` | `dd.remote.chaos.experiments` |
| `CHAOS_EVENTS_SUBJECT` | `dd.remote.chaos.events` |
| `CHAOS_PROBE_SUBJECT` | `dd.remote.chaos.probe` (queue group `dd-chaos-probe`) |

## HTTP

```
GET /healthz   liveness
GET /readyz    readiness (enabled/dryRun/NATS state)
GET /metrics   Prometheus text (dd_chaos_armed, pod_kills_total, probe_rtt_seconds, …)
```

## Config (env)

| Var | Default | Meaning |
|---|---|---|
| `CHAOS_ENABLED` | `false` | master kill switch |
| `CHAOS_DRY_RUN` | `true` | log-only even when enabled |
| `CHAOS_NAMESPACES` | `ai-ml,dd-dev` | namespaces to target |
| `CHAOS_BLAST_RADIUS` | `1` | max pods killed per tick |
| `CHAOS_POD_KILL_INTERVAL_SECONDS` | `300` | pod-kill cadence |
| `CHAOS_JITTER_ENABLED` | `false` | enable deployment jitter |
| `CHAOS_JITTER_DEPLOYMENTS` | _(empty)_ | `namespace/name` targets, comma-separated |
| `CHAOS_JITTER_HOLD_SECONDS` | `60` | how long a replica stays removed |
| `CHAOS_PROBE_INTERVAL_SECONDS` | `60` | NATS RTT probe cadence |

## Hardening

Beyond the kill switch / dry-run / blast-radius / protected-namespace guards above:

- **Never self-targets**: the pod-kill loop skips any pod whose name equals its own
  hostname (the pod name in Kubernetes), in addition to the `chaos-protected` label.
- **Selector is URL-encoded**: the operator-supplied `CHAOS_POD_LABEL_SELECTOR` is
  percent-encoded before going into the API query, so it can't malform or inject into the URL.
- **Bounded list**: pod listings request `limit=500` (first page only — chaos samples a
  victim, it doesn't need full coverage), bounding memory/latency on large clusters.
- **NATS resilience**: bounded connect-retry (`CHAOS_NATS_CONNECT_ATTEMPTS`); the k8s
  loops still run if NATS never comes up (experiments/events just aren't published).
- **Jitter restores on a fresh client** after the hold, in case the SA token/CA rotated
  during the window.

## Build & deploy

```bash
# from the k8s-cluster repo root
docker build -f remote/deployments/chaos-rs/Dockerfile -t dd-chaos:dev .
kubectl apply -k remote/deployments/chaos-rs/k8s
```

It deploys **disarmed** (`CHAOS_ENABLED=false`); only the read-only NATS probe is active
until you arm it.
