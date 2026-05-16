# `remote/idle-reaper-rs`

Tiny Rust worker that runs cluster maintenance loops:

- idle sweep: `POST /api/admin/remote-dev/reaper/sweep`
- cluster doctor: `POST /tasks` on `dd-dev-server-api` every 90 minutes
- NATS watchdog: copy-subscribe to task/event subjects and backstop worker
  prepare plus Gleam websocket fanout
- worker-image cron: build `docker.io/library/dd-dev-server:dev` every day at
  4am America/New_York from the latest `dev` branch

## Env

| Var | Required | Default |
|---|---|---|
| `REAPER_SWEEP_URL` | no | disabled |
| `REAPER_SECRET` | no | disabled |
| `REAPER_INTERVAL_SECONDS` | no | `60` |
| `REAPER_TIMEOUT_SECONDS` | no | `25` |
| `REAPER_DRY_RUN` | no | `false` |
| `NATS_WATCH_ENABLED` | no | `false` |
| `NATS_URL` | no | `nats://dd-nats.messaging.svc.cluster.local:4222` |
| `NATS_WATCH_TASK_SUBJECT` | no | `dd.remote.thread.*.tasks` |
| `NATS_WATCH_EVENT_SUBJECT` | no | `dd.remote.events` |
| `NATS_WATCH_REST_API_URL` | no | `http://dd-remote-rest-api.default.svc.cluster.local:8082` |
| `NATS_WATCH_GLEAM_BROADCAST_URL` | no | `http://dd-gleamlang-server.default.svc.cluster.local:8081/broadcast` |
| `NATS_WATCH_GLEAM_BROADCAST_SECRET` | yes, when enabled | — |
| `NATS_WATCH_ACTIVE_INTERVAL_SECONDS` | no | `5` |
| `NATS_WATCH_IDLE_INTERVAL_SECONDS` | no | `15` |
| `CLUSTER_DOCTOR_ENABLED` | no | `false` |
| `CLUSTER_DOCTOR_INTERVAL_SECONDS` | no | `5400` |
| `CLUSTER_DOCTOR_RUN_ON_START` | no | `false` |
| `CLUSTER_DOCTOR_TASK_URL` | no | `http://dd-dev-server-api.default.svc.cluster.local:8080/tasks` |
| `CLUSTER_DOCTOR_SERVER_AUTH_SECRET` | yes, when enabled | — |
| `CLUSTER_DOCTOR_THREAD_ID` | no | unset |
| `CLUSTER_DOCTOR_THREAD_TITLE` | no | `cluster telemetry doctor` |
| `CLUSTER_DOCTOR_PROVIDER` | no | dev-server default |
| `CLUSTER_DOCTOR_USER_ID` | no | unset |
| `WORKER_IMAGE_BUILD_ENABLED` | no | `false` |
| `WORKER_IMAGE_BUILD_TIMEZONE` | no | `America/New_York` |
| `WORKER_IMAGE_BUILD_HOUR` | no | `4` |
| `WORKER_IMAGE_BUILD_MINUTE` | no | `0` |
| `WORKER_IMAGE_BUILD_REPO_DIR` | no | `/opt/dd-next-1` |
| `WORKER_IMAGE_BUILD_REPO_URL` | no | `git@github.com:ORESoftware/k8s-cluster.git` |
| `WORKER_IMAGE_BUILD_REF` | no | `dev` |
| `WORKER_IMAGE_BUILD_IMAGE` | no | `docker.io/library/dd-dev-server:dev` |
| `WORKER_IMAGE_BUILD_NERDCTL` | no | `/usr/local/bin/nerdctl` |
| `WORKER_IMAGE_BUILD_GITHUB_DEPLOY_KEY` | yes, when enabled | — |
| `WORKER_IMAGE_BUILD_RUN_ON_START` | no | `false` |

The cluster doctor prompt is inline in `src/main.rs` for now. It tells the
agent to inspect Prometheus, Loki, Grafana, NATS, OTel, and runtime service
health, then make a narrow repo change and rely on `remote/dev-server` to push
and open/reuse the GitHub PR.

The worker-image cron stays in this deployment rather than a separate cron pod
so the runtime has one maintenance supervisor. It uses Rust scheduling rather
than Linux `cron`/`at`; the pod still mounts the EC2 containerd socket and
`nerdctl` binary so the build lands in the local `k8s.io` image store as
`docker.io/library/dd-dev-server:dev`.

## Build

```bash
docker build -t dd-idle-reaper:latest .
```
