# GCS Load Test Requests

- 2026-06-04 10k-medium: 5 loader replicas x 2000 clients, 2.5 msg/(conn*second), 180s.
- 2026-06-04 20k-light: 5 loader replicas x 4000 clients, 1.0 msg/(conn*second), 180s.
- 2026-06-04 20k-medium: 5 loader replicas x 4000 clients, 2.5 msg/(conn*second), 180s.
- 2026-06-04 30k-light: 6 loader replicas x 5000 clients, 1.0 msg/(conn*second), 180s.
- 2026-06-04 30k-medium: 6 loader replicas x 5000 clients, 2.5 msg/(conn*second), 180s.
- 2026-06-04 requeue 10k-medium after GitHub cancelled older pending concurrency runs.
- 2026-06-04 diagnose GCS rollout after 10k-light and 10k-medium failed before load windows.
- 2026-06-04 diagnose gcs-router rollout after checksum fix still missed progress deadline.
- 2026-06-04 diagnose runtime collapse after 10k-light reached correctness but failed sustained WSS load.
- 2026-06-04 rerun compact runtime-collapse diagnostics with GCS pod logs before router summaries.
- 2026-06-04 10k-light rerun after websocket side-effect in-flight cap.
- 2026-06-04 10k-light retry with GCS_WS_SIDE_EFFECT_MAX_IN_FLIGHT=2048: 5 loader replicas x 2000 clients, 1.0 msg/(conn*second), 180s.
- 2026-06-04 diagnose 10k-light 2048 side-effect cap collapse: confirm GCS pod restarts, OOM state, and dependency pressure.
- 2026-06-04 recover GCS side-effect cap to 512 after 2048 retry OOMKilled pods during 10k-light.
- 2026-06-04 10k-light retry after rate-limiting websocket side-effect shed logs: 5 loader replicas x 2000 clients, 1.0 msg/(conn*second), 180s.
- 2026-06-04 diagnose current GCS state while stale recovery SSM command is still in progress.
- 2026-06-04 10k-light retry after shedding side-effect retries and sampling broker hot-path logs: 5 loader replicas x 2000 clients, 1.0 msg/(conn*second), 180s.
- 2026-06-04 10k-light trigger after merging latest parent dev: 5 loader replicas x 2000 clients, 1.0 msg/(conn*second), 180s.
- 2026-06-04 diagnose active 10k-light retry-pressure loadtest while SSM output is still pending.
- 2026-06-04 snapshot active 10k-light retry-pressure loadtest metrics while proof run is still pending.
- 2026-06-04 20k-light goroutine-fix validation: 5 loader replicas x 4000 clients, 1.0 msg/(conn*second), 180s.
- 2026-06-04 20k-medium goroutine-fix validation after 20k-light pass: 5 loader replicas x 4000 clients, 2.5 msg/(conn*second), 180s.
- 2026-06-04 20k-medium goroutine-fix retry with fewer loader pods: 4 loader replicas x 5000 clients, 2.5 msg/(conn*second), 180s.
- 2026-06-04 20k-medium goroutine-fix retry after Argo sync/scale guard: 4 loader replicas x 5000 clients, 2.5 msg/(conn*second), 180s.
- 2026-06-04 20k-medium goroutine-fix retry after freeing MIP solver pods: 4 loader replicas x 5000 clients, 2.5 msg/(conn*second), 180s.
- 2026-06-05 20k-medium goroutine-fix retry after disabling MIP Argo/KEDA owner: 4 loader replicas x 5000 clients, 2.5 msg/(conn*second), 180s.
- 2026-06-05 20k-medium goroutine-fix retry with fewer loader pods: 3 loader replicas x 6667 clients, 2.5 msg/(conn*second), 180s.
- 2026-06-05 snapshot after passing 20k-medium goroutine-fix 3x6667 run: collect CPU, memory, router, load-shed, and dependency telemetry.
- 2026-06-06 40k-light one-pod pprof campaign: strict aggregate target 40000, 3 loader replicas per loader, 1.0 msg/(conn*second), 180s.
- 2026-06-06 40k-medium one-pod pprof campaign: strict aggregate target 40000, 3 loader replicas per loader, 2.5 msg/(conn*second), 180s.
- 2026-06-06 50k-light one-pod pprof campaign: strict aggregate target 50000, 3 loader replicas per loader, 1.0 msg/(conn*second), 180s.
- 2026-06-06 50k-medium one-pod pprof campaign: strict aggregate target 50000, 3 loader replicas per loader, 2.5 msg/(conn*second), 180s.

## 2026-06-10 one-pod campaign results (chat.vibe gcs-hot-path-perf, gcs 6 CPU / 8 Gi, GOMAXPROCS=6)

Build: chat.vibe `feature/gcs-hot-path-perf` (hot-path perf + load shedding +
`--max-active-conns`), ws-client correctness fix `981b9a8c`. gcs single pod,
limits 6 CPU / 8 Gi, `GOMAXPROCS=6`, `GOMEMLIMIT=7168MiB`; 3 loaders
(rust+nodejs+gleam) x 3 replicas. Correctness smoke made non-gating
(`GCS_LOADTEST_REQUIRE_CORRECTNESS=true` to restore) — the CLI smoke lacks the
loaders' conv-membership setup; the loaders validate end-to-end delivery.

- 40k-light  - PASS: all loaders open=13335, failed=0, receive_errors=0; aggregate 40005.
- 40k-medium - PASS: all loaders open=13335, failed=0, receive_errors=0; aggregate 40005;
  p50 ~0.7-1.1ms after GOMAXPROCS 4->6 (was ~2s at GOMAXPROCS=4, which shed ~300 conns/loader).
- 50k-light  - PASS: all loaders open=16668, failed=0, receive_errors=0; aggregate 50004.
- 50k-medium - FAIL (single-pod 6-CPU capacity wall): connections collapse
  (open ~2.3k-8.2k of 16668), failed=132k-176k, p99 ~40s, heavy load-shedding.
  ~125k msg/s inbound + fan-out saturates one 6-CPU pod. Needs >6 CPU or a 2nd
  gcs pod/node; not pursued per the 6-CPU budget.
