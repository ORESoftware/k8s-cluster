# `remote/deployments/ai-ml-pipeline`

Python3 online feature pipeline for the remote AI/ML stack.

It is intentionally small enough to run next to the existing Rust MDP/POMDP/RL optimizer:

1. Raw telemetry arrives through `POST /ingest` or `dd.remote.telemetry.raw`.
2. The service normalizes metrics into ML features, EWMA baselines, z-scores, risk states, and
   action impact hints.
3. It publishes enriched feature events on `dd.remote.ml.features`.
4. It publishes MDP-ready telemetry on `dd.remote.telemetry.mdp`, where `dd-mdp-optimizer` can run
   value iteration and choose a policy action.

This keeps Python focused on data and ML feature work while Rust remains the deterministic
optimizer.

## Endpoints

- `GET /healthz`
- `GET /readyz`
- `GET /metrics`
- `GET /status` - requires `X-Server-Auth` or `Auth`
- `POST /analyze` - score telemetry without publishing
- `POST /ingest` - score telemetry and publish feature, MDP, and runtime events
- `POST /mdp/features` - return only the MDP telemetry request body

`GET /healthz` and `GET /metrics` stay open for Kubernetes probes and Prometheus scraping. All
other HTTP routes require `SERVER_AUTH_SECRET` through `X-Server-Auth` or `Auth`; the service exits
at startup unless `SERVER_AUTH_SECRET` is configured or `ML_ALLOW_UNAUTHENTICATED=true` is set
explicitly for local development.

Example:

```bash
curl -s http://localhost:8099/analyze \
  -H 'content-type: application/json' \
  -d '{
    "requestId": "demo",
    "service": "dd-dev-server-api",
    "scope": "app",
    "metrics": {
      "p95LatencyMs": 920,
      "errorRate": 0.03,
      "queueDepth": 75
    }
  }'
```

## NATS subjects

- `ML_RAW_TELEMETRY_SUBJECT=dd.remote.telemetry.raw`
- `ML_FEATURE_SUBJECT=dd.remote.ml.features`
- `ML_MDP_TELEMETRY_SUBJECT=dd.remote.telemetry.mdp`
- `ML_EVENT_SUBJECT=dd.remote.events`
- `ML_DEAD_LETTER_SUBJECT=dd.remote.ml.deadletter`

## Container image

The EC2 Kubernetes overlay runs `docker.io/library/dd-ai-ml-pipeline:dev` from the image contents
instead of mounting the repo checkout. Build it from the k8s-cluster repo root so the Dockerfile can
bake in the generated API docs and generated Python NATS subject constants:

```bash
docker build -f remote/deployments/ai-ml-pipeline/Dockerfile \
  -t docker.io/library/dd-ai-ml-pipeline:dev .
```

On the EC2 node/containerd path, build the first-party AI/ML images in one pass before syncing
Argo CD:

```bash
bash remote/tools/build-ai-ml-platform-images.sh
```

Or build this service directly with the same tag in the `k8s.io` namespace:

```bash
nerdctl -n k8s.io build -f remote/deployments/ai-ml-pipeline/Dockerfile \
  -t docker.io/library/dd-ai-ml-pipeline:dev .
```

## Runtime model

The first model is an online statistical model rather than a heavyweight batch-trained artifact:

- Welford mean and variance per service/layer/signal
- EWMA baseline per signal
- threshold-aware risk scoring for latency, errors, queue lag, CPU, memory, restarts, and
  availability
- simple transition counting from previous observed state/action to current state
- action impact hints for the MDP/RL optimizer

The in-memory model has bounded cardinality through `ML_MAX_TRACKED_SERIES` and
`ML_MAX_TRANSITION_KEYS`, validates telemetry windows and signal weights before publishing to the
MDP optimizer, redacts credentials from status output, waits for NATS flush acknowledgements, rejects
oversized NATS publishes, and sends malformed NATS messages to `dd.remote.ml.deadletter` without
copying raw payload content.

The Kubernetes bundle also carries JSON schemas for the raw ingest and MDP telemetry shapes in
`dd-ai-ml-data-contracts`, so Dagster/Airflow/Spark jobs can generate the same contract the online
service expects.

That gives the cluster a real data pipeline today, while leaving room for MLflow-registered models,
Dagster/Airflow jobs, Spark features, or LlamaIndex retrieval steps to replace individual scoring
functions later.
