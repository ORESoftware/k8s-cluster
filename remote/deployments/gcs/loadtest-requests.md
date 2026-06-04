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
