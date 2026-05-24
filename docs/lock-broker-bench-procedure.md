# Lock-broker benchmark procedure

Head-to-head correctness + performance comparison of three mutex
brokers in this cluster:

| Service                       | Source                                                                 | Variant   |
|-------------------------------|------------------------------------------------------------------------|-----------|
| `dd-rust-network-mutex`       | `remote/submodules/rust-network-mutex-rs`                              | Rust port |
| `dd-live-mutex-submodule`     | `remote/submodules/live-mutex` branch `feat/sweeper-fencing-acquire-many-http` | Node fork |
| `dd-live-mutex`               | `npm install live-mutex@0.2.25`                                        | upstream baseline |

All three speak the same NDJSON-over-TCP wire protocol. Each is
listening on port `6970` (TCP) inside `default`. The Rust port and
the submodule fork additionally expose an HTTP front-end on port
`6971` with `/healthz`, `/metrics`, an HTML status page, and the
in-process bridge for HTTP-driven lock operations.

## Load testers

| Service                       | Runtime  | Source                                          | Trigger surface          |
|-------------------------------|----------|-------------------------------------------------|--------------------------|
| `dd-lock-loadtest-trigger`    | Node.js  | `remote/deployments/live-mutex-loadtest-node`   | `POST /runs` on `:8110`  |
| `dd-lock-loadtest-rs`         | Rust     | `remote/deployments/lock-loadtest-rs`           | `POST /runs` on `:8120`  |
| `dd-lock-loadtest-gleam`      | Gleam/BEAM | `remote/deployments/lock-loadtest-gleam`      | env-driven; stdout JSON  |

The Node and Rust testers expose a parameterised HTTP API: the
caller picks the broker per-request via `brokerHost` / `brokerPort`
in the body. The Gleam tester picks its target from `BROKER_HOST`
and runs benchmarks on a fixed schedule, emitting one JSON line per
window to stdout.

## Wiring (one-time)

The three Argo apps already exist in the repo. After pulling, apply
each from the operator's bastion (`dd-bastion`) where the kube-API
is reachable:

```bash
# From the cluster operator's machine (the WireGuard VPN must be up
# so kubernetes.default.svc resolves; see remote/argocd/vpn/readme.md).
kubectl apply -f remote/argocd/apps/dd-lock-loadtest-rs.application.yaml
kubectl apply -f remote/argocd/apps/dd-lock-loadtest-gleam.application.yaml
```

`dd-lock-loadtest-trigger` (Node) is already applied in the cluster.

The submodule broker (`dd-live-mutex-submodule`) is bundled into the
`dd-next-runtime` ArgoCD app, which auto-syncs on every `dev`-branch
commit. Its hostPath mount needs the submodule initialised on the
EC2 host once:

```bash
# On the EC2 node that backs the deployment (see
# remote/argocd/dd-next-runtime/readme.md for the operator runbook):
cd /home/ec2-user/codes/dd/dd-next-1
git submodule update --init --recursive remote/submodules/live-mutex
```

After that, `kubectl rollout restart deploy/dd-live-mutex-submodule`
will pick the submodule up on the next pod boot.

## Health-check the brokers

```bash
# All three should answer cleanly.
kubectl exec -it deploy/dd-bastion -- /bin/sh -lc '
  curl -s http://dd-rust-network-mutex.default.svc.cluster.local:6971/healthz; echo
  curl -s http://dd-live-mutex-submodule.default.svc.cluster.local:6971/healthz; echo
  # Upstream npm broker has no HTTP front-end; TCP-probe instead.
  printf "{\"type\":\"version\",\"uuid\":\"v\",\"value\":\"0.2.25\"}\n" | \
    nc -w 2 dd-live-mutex.default.svc.cluster.local 6970
'
```

The HTML status page on the Rust + submodule brokers is also worth
opening through the dev-server browser harness; both render counters
for `holders`, `pendingDeadlines`, `ttlEvictionsTotal`, etc.

## Trigger a single benchmark

Run a 60-second 16-worker bench against each broker, sequentially.
Store the results so we have a snapshot.

```bash
RUN_DIR=/tmp/lock-bench-$(date +%Y%m%d-%H%M%S)
mkdir -p "$RUN_DIR"
trap 'echo "results in $RUN_DIR"' EXIT

for BROKER in dd-rust-network-mutex dd-live-mutex-submodule dd-live-mutex; do
  echo "=== $BROKER (Rust load tester) ==="
  curl -s -X POST http://dd-lock-loadtest-rs.default.svc.cluster.local:8120/runs \
    -H 'content-type: application/json' \
    -d "{
      \"brokerHost\": \"$BROKER.default.svc.cluster.local\",
      \"brokerPort\": 6970,
      \"durationSeconds\": 60,
      \"workers\": 16,
      \"keys\": 32,
      \"ttlMs\": 4000
    }"
  # Wait the run window plus a 30s aggregation pad.
  sleep 95
  curl -s http://dd-lock-loadtest-rs.default.svc.cluster.local:8120/runs/last \
    | tee "$RUN_DIR/loadtest-rs.$BROKER.json"
done
```

For the Node tester, the same pattern but pointed at the `:8110`
service (see `remote/deployments/live-mutex-loadtest-node/README.md`
for body schema). For the Gleam tester, set `BROKER_HOST` and
`kubectl logs -f deploy/dd-lock-loadtest-gleam | jq -c 'select(.type=="bench-summary")'`
captures the same data.

## What to compare

### Performance

| Field                       | Lower is better | Why |
|-----------------------------|-----------------|-----|
| `acquireLatencyUsP50`       | yes             | Median round-trip. |
| `acquireLatencyUsP99`       | yes             | Tail latency. The improvements branch's centralised TTL sweeper should keep this stable under load. |
| `acquireLatencyUsMax`       | yes             | Worst case observed; useful for spotting GC pauses or the occasional slow grant after a long queue. |

| Field                       | Higher is better |
|-----------------------------|------------------|
| `actualRps`                 | overall throughput. |
| `acquired`                  | total successful grants. |

The three brokers should NOT be radically different at low load. At
high load (16 workers, 32 keys), expect:

- `dd-rust-network-mutex` to win on `actualRps` and tail latency
  (Tokio + Rust's lower per-frame allocation cost).
- `dd-live-mutex-submodule` to be in the same ballpark at the median
  but to show heavier tails (Node's GC).
- `dd-live-mutex` (npm baseline) to be the slowest at high load —
  per-holder `setTimeout` for TTL eviction adds work proportional to
  the number of in-flight holders, vs the single-sweeper design on
  the other two.

### Correctness

| Field                | Expected | Why |
|----------------------|----------|-----|
| `failedAcquires`     | 0        | A bench within the broker's capacity should never fail to acquire. |
| `failedReleases`     | 0        | Same. Releases that hit `unlocked: false` are real bugs. |
| `fencingViolations`  | 0        | Per-key strict monotonicity is a correctness invariant. |

The Rust load tester explicitly checks fencing-token monotonicity
across all workers. A non-zero `fencingViolations` is a real
regression, not a flakiness signal — investigate before continuing.

## Operational notes

- The hostPath build pattern means the broker pods need the EC2
  checkout to be on the right branch. Confirm the symlink in
  `/home/ec2-user/codes/dd/dd-next-1` points to a checkout where
  `git status` is clean and `git rev-parse HEAD` returns a commit
  that includes the improvements branch.
- The `dd-live-mutex` (npm) baseline is intentionally untouched.
  Don't let GitOps drift override its `live-mutex@0.2.25` pin —
  changing that turns it into a second copy of the submodule fork.
- Grafana dashboards can plot
  `lock_loadtest_acquire_latency_us_{p50,p95,p99,max}` and
  `lock_loadtest_last_rps` directly; the tester's `BrokerLabel`
  shortens the broker hostname to `dd-foo:6970` so legends stay
  readable.

## Known issues

- `useAcquireMany: true` against the Rust broker hangs under
  contention. The grant phase emits a follow-up frame with the same
  `uuid` once the queued composite request becomes head, but our
  broker's queue-pop logic doesn't always re-fire when one of the
  contended keys frees. Tracked separately; for the head-to-head
  bench, leave `useAcquireMany: false` (default).
- The Gleam tester does not assert per-key strict monotonicity
  across worker boundaries. It exposes per-key high-water marks
  via `uniqueKeysObserved` but inter-worker monotonicity assertions
  belong on the Rust tester (where worker stats merge under a
  single mutex).
