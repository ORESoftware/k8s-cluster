# GCS Load Test Requests

- 2026-06-04 10k-medium: 5 loader replicas x 2000 clients, 2.5 msg/(conn*second), 180s.
- 2026-06-04 20k-light: 5 loader replicas x 4000 clients, 1.0 msg/(conn*second), 180s.
- 2026-06-04 20k-medium: 5 loader replicas x 4000 clients, 2.5 msg/(conn*second), 180s.
- 2026-06-04 30k-light: 6 loader replicas x 5000 clients, 1.0 msg/(conn*second), 180s.
- 2026-06-04 30k-medium: 6 loader replicas x 5000 clients, 2.5 msg/(conn*second), 180s.
- 2026-06-04 requeue 10k-medium after GitHub cancelled older pending concurrency runs.
- 2026-06-04 diagnose GCS rollout after 10k-light and 10k-medium failed before load windows.
