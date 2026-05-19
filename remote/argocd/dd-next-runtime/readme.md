# `remote/argocd/dd-next-runtime`

GitOps manifests for the baseline runtime that should always be visible in Argo:

- `dd-remote-web-home` (Rust web layer for `/` + `/home`)
- `dd-remote-auth` (Rust PIN auth service that sets the gateway `dd_auth` cookie)
- `dd-remote-rest-api` (Rust RDS/Postgres REST API for agent task data)
- `dd-agent-worker-broker` (Rust worker-dispatch broker for NATS-first, direct-if-awake handoff)
- `dd-container-pool` (Rust Postgres-configured warm container pool over HTTP or NATS)
- `dd-build-server` (Rust CI/CD build server for repo image builds and controlled k8s deploys)
- `dd-gleam-lambda-runner` (Gleam child-process runner for user-defined lambda invocations)
- `dd-remote-queue-consumer` (Rust NATS shadow consumer that prepares UUID-bound thread workers)
- `dd-webrtc-signaling` (Rust WebRTC room signaling over WebSocket)
- `dd-web-scraper` (Node.js/Fastify scraping worker with browser and DOM strategies)
- `dd-live-mutex` (single-broker Live-Mutex TCP service for cluster-local locking)
- `dd-ai-ml-pipeline` (Python3 online telemetry feature pipeline in the `ai-ml` namespace)
- `dd-des-simulator` (Rust asynchronous discrete event simulation service with `des.v1` model validation)
- `dd-contract-service` (Rust Solana contract gateway for `solana.contract.v1` validation)
- `dd-trading-server` (Rust trading decision service for `trading.decision.v1` risk-gated order intents)
- `dd-dev-server-api` (bootstrap Node.js coding-agent task manager for `/tasks`, `/stream`,
  `/status`, `/agents`, `/healthz`)
- `dd-redis-cache` (cluster-local Redis cache for low-latency ephemeral runtime state)
- `dd-remote-gateway` (public k8s path splitter on host ports 80 and 443)
- `dd-idle-reaper` (Rust scheduler for idle sweeps and the 90-minute cluster doctor task)

The ArgoCD application for this bundle is
`remote/argocd/apps/dd-next-runtime.application.yaml`. Keep Kubernetes object changes in Git and
let Argo sync them; the manual GitHub Actions workflow only refreshes the current node-local
`dd-remote-web-home:dev` image while that image still lives in EC2 containerd instead of a registry.

## Live-Mutex broker

`dd-live-mutex` runs the published `live-mutex@0.2.25` package from
`docker.io/library/node:22-bookworm-slim` and starts the package-provided `lmx_start_server`
broker on port `6970`. The upstream project does not currently publish a
`docker.io/oresoftware/live-mutex-broker:latest` image, so the Deployment avoids that missing image
reference.

The broker is exposed only inside the cluster at `dd-live-mutex.default.svc.cluster.local:6970`.
Live-Mutex is a single-source-of-truth broker, so this workload intentionally stays at
`replicas: 1` with `strategy: Recreate`; Kubernetes kills the existing pod before creating a
replacement during rollouts instead of temporarily running two brokers.

## Redis cache

`dd-redis-cache` runs an in-cluster Redis cache at
`dd-redis-cache.default.svc.cluster.local:6379`. It is intentionally ephemeral: append-only files
and snapshot saves are disabled, and Redis evicts keys with `allkeys-lru` after the configured
`256mb` maxmemory ceiling. Keep durable state in Postgres or the service-specific source of truth;
use this service only for hot runtime cache entries that can be rebuilt.

The cache is network-isolated by `dd-redis-cache.networkpolicy.yaml`. In-namespace clients must opt
in with the pod label `dd.dev/redis-cache-client: 'true'` before they can connect to TCP `6379`.
Redis runs as UID/GID `999`, with no service-account token, a read-only root filesystem, dropped
Linux capabilities, `RuntimeDefault` seccomp, and writable `emptyDir` mounts only for `/data` and
`/tmp`. The ACL file keeps the unauthenticated default user for cluster-local cache ergonomics, but
blocks administrative, dangerous, scripting, module, persistence, and bulk-key commands.

Gateway path map:

- `/`, `/home` -> `dd-remote-web-home:8080`
- `/auth` -> `dd-remote-auth:8083`
- `/agents/tasks` -> `dd-remote-web-home:8080`
- `/api/agents/tasks` -> `dd-remote-rest-api:8082`
- `/api/agents/threads/<uuid>/prepare` -> `dd-remote-rest-api:8082` (internal auth required)
- `POST /api/agent-worker/threads/<uuid>/tasks` -> `dd-agent-worker-broker:8098`
- `/container-pools`, `/container-pools/<pool>`,
  `POST /container-pools/<pool>/dispatch` -> `dd-container-pool:8102`
  (internal auth required)
- `/builds`, `/builds/<jobId>`, `/builds/<jobId>/logs` -> `dd-build-server:8100`
  (internal auth required)
- `/lambdas/functions` -> `dd-remote-web-home:8080`
- `/api/lambdas/functions` -> `dd-remote-rest-api:8082`
- `POST /lambdas/invoke/<function-id>` -> `dd-gleam-lambda-runner:8083` directly
- `/webrtc/`, `/webrtc/healthz`, `/webrtc/metrics`, `/webrtc/signal` -> `dd-webrtc-signaling:8095`
- `/des/`, `/des/model/schema`, `/des/model/example`, `POST /des/validate`,
  `POST /des/simulate`, `/des/simulations/<jobId>` -> `dd-des-simulator:8099`
- `/contracts/`, `/contracts/schema`, `/contracts/example`, `POST /contracts/validate`,
  `POST /contracts/simulate`, `POST /contracts/send` -> `dd-contract-service:8101`
  (internal auth required)
- `/ml/`, `/ml/healthz`, `/ml/metrics`, `/ml/status`, `POST /ml/analyze`, `POST /ml/ingest` ->
  `dd-ai-ml-pipeline.ai-ml:8099` (internal auth required)
- `/trading/`, `/trading/schema`, `/trading/example`, `POST /trading/decide` ->
  `dd-trading-server:8103` (internal auth required)
- `/scrape`, `/scrape/strategies`, `/scrape/healthz`, `/scrape/metrics` -> `dd-web-scraper:8097`
  (internal auth required)
- `/tasks`, `/stream`, `/status`, `/agents`, `/healthz` -> bootstrap `dd-dev-server-api:8080`
- `/dd-thread/<short>/...` -> target per-thread Kubernetes Ingress shape; the selected Node.js
  worker is pinned to one thread and does not route UUIDs itself. `/dd-thread/<short>/ws` is the
  direct worker WebSocket for replay/live task events.

The Node.js worker image is pre-baked as `docker.io/library/dd-dev-server:dev` on the EC2
containerd node. It already contains git, OpenSSH, GitHub CLI, provider CLIs, the compiled
`remote/dev-server` server, and a warm `dd-next-1` checkout template. The container runs as the
built-in `node` user; mounted workspaces live under `/home/node/workspace`.

Protected ops paths accept either the legacy `Auth` request header or the browser `dd_auth` cookie.
Browser document requests redirect to `/auth?return=<original path>`; API/curl callers still
receive the redacted JSON `{"error":"unauthorized","errMessage":"missing required dd header"}`
response. The PIN and cookie value must be provided by the `dd-remote-auth-secrets` Kubernetes
secret, not committed to Git.

## Gateway TLS

`dd-remote-gateway` terminates HTTPS itself with a Kubernetes TLS secret named
`dd-remote-gateway-tls` in the `default` namespace. HTTP remains enabled for ACME HTTP-01
challenge renewal and redirects browser traffic to HTTPS.

Create or rotate the self-signed certificate on the EC2 host before applying the gateway
deployment:

```bash
mkdir -p /home/ec2-user/dd-gateway-tls
openssl req -x509 -nodes -newkey rsa:2048 -days 365 \
  -keyout /home/ec2-user/dd-gateway-tls/tls.key \
  -out /home/ec2-user/dd-gateway-tls/tls.crt \
  -subj "/CN=54.91.17.58" \
  -addext "subjectAltName=IP:54.91.17.58,DNS:ec2-54-91-17-58.compute-1.amazonaws.com,DNS:localhost,IP:127.0.0.1"
kubectl create secret tls dd-remote-gateway-tls \
  --cert=/home/ec2-user/dd-gateway-tls/tls.crt \
  --key=/home/ec2-user/dd-gateway-tls/tls.key \
  -n default \
  --dry-run=client -o yaml | kubectl apply -f -
```

Browsers will warn because the certificate is self-signed. CLI checks should use
`curl -k https://54.91.17.58/home` until a real CA-backed certificate is installed.

For a trusted Let's Encrypt certificate on the bare EC2 IP, use Certbot 5.4+ and the short-lived
IP-address profile. On Amazon Linux 2023 this currently means installing Python 3.12 and a
user-owned Certbot virtualenv. The gateway serves `/.well-known/acme-challenge/` from the host path
`/home/ec2-user/dd-acme-webroot`, so Certbot can use webroot mode without taking port 80 down:

```bash
mkdir -p /home/ec2-user/dd-acme-webroot/.well-known/acme-challenge
sudo dnf install -y python3.12 python3.12-pip
python3.12 -m venv /home/ec2-user/certbot-venv-312
/home/ec2-user/certbot-venv-312/bin/python -m pip install --upgrade pip setuptools wheel
/home/ec2-user/certbot-venv-312/bin/python -m pip install 'certbot>=5.4,<6'
/home/ec2-user/certbot-venv-312/bin/certbot certonly \
  --config-dir /home/ec2-user/letsencrypt/config \
  --work-dir /home/ec2-user/letsencrypt/work \
  --logs-dir /home/ec2-user/letsencrypt/logs \
  --preferred-profile shortlived \
  --webroot \
  --webroot-path /home/ec2-user/dd-acme-webroot \
  --ip-address 54.91.17.58 \
  --agree-tos \
  --register-unsafely-without-email
bash /home/ec2-user/codes/dd/dd-next-1/remote/ec2/renew-letsencrypt-gateway-cert.sh deploy
```

IP-address certificates are intentionally short-lived, so renewal needs a deploy hook that rewrites
`dd-remote-gateway-tls` and restarts the gateway. Run
`remote/ec2/renew-letsencrypt-gateway-cert.sh renew` from cron or another scheduler several times
per day. The EC2 bootstrap path should install `remote/ec2/dd-letsencrypt-renew.timer`, which runs
the renewal service every six hours with jitter and lets Certbot skip when the cert is not yet due.

## REST API database secret

The REST API deployment expects optional Kubernetes secrets generated by the External Secrets
Operator path in `remote/argocd/secrets/`:

- `dd-agent-secrets`
- `dd-remote-rest-api-secrets`
- `dd-gleam-lambda-runner-secrets`

Use `dd-agent-secrets` for shared remote runtime values:

- `SERVER_AUTH_SECRET`
- `DD_REPO_URL` and `DD_REPO_REF` for the repo/base branch that the bootstrap Node.js worker is
  pinned to at runtime
- model-provider keys like `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`, and `OPENAI_API_KEY`
- GitHub credentials used by the remote dev worker entrypoint and PR creation path:
  `GH_DEPLOY_KEY`, optional `GH_DEPLOY_KEY_PUBLIC`, and optional `GH_PAT`

GitHub deploy keys must stay in Kubernetes/AWS secrets and must never be baked into
`dd-dev-server` or any container-pool image. The worker entrypoint writes `GH_DEPLOY_KEY` to
`GH_DEPLOY_KEY_PATH` at startup with `0600` permissions, then uses it through `GIT_SSH_COMMAND` for
clone/fetch/push. A safe AWS Secrets Manager value for `dd/remote-dev/agent-secrets` looks like:

```json
{
  "SERVER_AUTH_SECRET": "replace-with-long-random-secret",
  "DD_REPO_URL": "git@github.com:ORESoftware/live-mutex.git",
  "DD_REPO_REF": "dev",
  "GH_DEPLOY_KEY": "-----BEGIN OPENSSH PRIVATE KEY-----\n...\n-----END OPENSSH PRIVATE KEY-----",
  "GH_DEPLOY_KEY_PUBLIC": "ssh-ed25519 ... comment",
  "GH_PAT": "optional-fine-grained-token-for-gh-cli-prs",
  "ANTHROPIC_API_KEY": "replace-me"
}
```

Use `dd-remote-rest-api-secrets` for RDS-specific values:

- `AGENT_TASKS_RDS_DATABASE_URL`
- `RDS_DATABASE_URL`

`dd-trading-server` also reads `RDS_DATABASE_URL`/`AGENT_TASKS_RDS_DATABASE_URL` so it can load
broker/platform metadata from the generic `app_config` row `default/trading.platforms.v1`. Seed
that row with `remote/databases/pg/seeds/trading-platform-app-config.sql`. Broker API credentials
and account IDs should stay in `dd-trading-broker-secrets`; the RDS row stores only platform
metadata and secret key names.

Use `dd-gleam-lambda-runner-secrets` for independent lambda runtime values:

- `LAMBDA_DATABASE_URL`
- `NATS_URL` if the runner subscribes or publishes runtime updates
- runtime-specific provider keys that user lambdas are allowed to consume

The web deployment serves HTML only and does not mount database secrets. Browser JavaScript calls
`/api/agents/tasks` and `/api/lambdas/functions` through the gateway directly. Lambda invocation
traffic uses `/lambdas/invoke/<function-id>` and goes from the gateway to the Gleam runner without
passing through the REST API. The runner reads `LAMBDA_DATABASE_URL` from
`dd-gleam-lambda-runner-secrets` and uses Postgres to resolve that immutable UUID to a lambda
definition.

Model-provider keys, GitHub credentials, gateway/shared-auth values, and REST database URLs should
be updated in AWS Secrets Manager, not in Git. The auth service consumes `DD_AUTH_PIN` and
`DD_AUTH_COOKIE_VALUE` from `dd-remote-auth-secrets`; rotate those values through AWS Secrets
Manager or the matching External Secrets path before applying this deployment. After rotation,
restart the deployments that consume the changed secret so env vars reload.

## Reaper and cluster doctor

The reaper deployment runs `remote/idle-reaper-rs`.

Idle sweep is enabled only when both values exist:

- name: `dd-idle-reaper-secret`
- namespace: `default`
- key: `REAPER_SECRET`
- config value: `REAPER_SWEEP_URL`

If the secret or `REAPER_SWEEP_URL` is not set, the pod stays healthy and logs that sweeping is
disabled.

Cluster doctor is configured in `dd-idle-reaper-config`:

- `CLUSTER_DOCTOR_ENABLED=true`
- `CLUSTER_DOCTOR_INTERVAL_SECONDS=5400`
- `CLUSTER_DOCTOR_TASK_URL=http://dd-dev-server-api.default.svc.cluster.local:8080/tasks`
- `dd-idle-reaper-secret` key `CLUSTER_DOCTOR_SERVER_AUTH_SECRET`
- `dd-idle-reaper-secret` key `NATS_WATCH_GLEAM_BROADCAST_SECRET`

Every 90 minutes it dispatches the inline prompt from `remote/idle-reaper-rs/src/main.rs` to
`dd-dev-server-api`. The agent inspects Prometheus, Loki, Grafana, NATS, and runtime service
health, then makes a narrow repo fix when there is an actionable issue. `remote/dev-server` pushes
the branch and opens/reuses the PR.

The same deployment also runs an adaptive NATS watchdog. It listens to copies of
`dd.remote.thread.*.tasks` and `dd.remote.events`; task messages idempotently call
`/api/agents/threads/<threadId>/prepare`, and event messages are reposted to the Gleam websocket
fanout endpoint. When a window has activity it checks again after 5 seconds; quiet windows back off
to 15 seconds.

The reaper is also the cron supervisor for the local worker image. Every day at 4am America/New_York
it fetches/fast-forwards the EC2 checkout, runs `nerdctl -n k8s.io build` for
`remote/dev-server`, and overwrites the local image tag `docker.io/library/dd-dev-server:dev`. New
thread workers use that tag via `imagePullPolicy: IfNotPresent`, so the next created pod picks up
the newest local image on the EC2 Kubernetes node. This is intentionally a Rust scheduler inside
the reaper deployment, not Linux `cron`/`at`; Kubernetes keeps the supervisor process alive, and
the deployment mounts the EC2 containerd socket plus `nerdctl` for the actual build.

`dd-headlamp-cron-sentinel` is a tiny native Kubernetes `CronJob` kept in this kustomization only
to make Headlamp's Jobs and Cron Jobs workload cards non-zero. It runs a no-op BusyBox pod and uses
`concurrencyPolicy: Forbid` so there is normally one active child `Job`; the real maintenance loops
above still live in `dd-idle-reaper`.

## NATS shadow prepare path

The runtime now includes a shadow NATS prepare path for future queue execution:

1. `dd-remote-rest-api` still performs direct dispatch to the selected thread worker.
2. After a direct dispatch succeeds, it publishes the task payload through JetStream to
   `dd.remote.thread.<threadId>.tasks` and also emits `dd.remote.orchestrator.wakeup`.
3. `dd-remote-queue-consumer` reads the durable pull consumer `dd-remote-thread-preparer` on the
   `DD_REMOTE_TASKS` stream.
4. KEDA watches that JetStream consumer lag and scales `dd-remote-queue-consumer` from 1 to 8 pods
   when pending messages build up, then returns to 1 after the stream drains.
5. The consumer calls the internal REST route `/api/agents/threads/<threadId>/prepare`, which
   creates/scales the deterministic `dd-thread-<short>` Deployment and waits for readiness.
6. The queue consumer stores taskId receipts under `/tmp/dd-remote-queue-consumer/tasks`; the
   Node.js worker also stores taskId receipts under its log directory. Repeated messages are
   accepted idempotently and do not start duplicate agent runs.

This proves the queue handoff and warmup behavior without allowing arbitrary generic workers to
steal coding-agent execution. Real queued `task.dispatch` messages are routed to repo-scoped
Node chat/Claude warm pools first, with direct REST fallback to the deterministic thread worker.

`dd-agent-worker-broker` is the additive long-run replacement for REST-owned worker dispatch. Its
first route, `POST /api/agent-worker/threads/<threadId>/tasks`, publishes the task to JetStream,
emits the wakeup subject, direct-posts to the deterministic Node.js worker only when that worker is
already healthy, and otherwise scales the worker Deployment to `1` while returning `202 queued`.

## Container pool

`dd-container-pool` is a Rust control surface for generic warm workers that do not need the
thread-affine Kubernetes Deployment shape. It reads the `app_config` row
`scope=default`, `key=container-pool.runtime-pools.v1` from Postgres, falls back to active
`container_pool_configs` rows, starts local containerd containers through `nerdctl -n k8s.io run -d`,
and keeps each pool between `min_warm` and `max_warm`. The EC2 deployment runs privileged with
`hostNetwork: true`, the containerd socket, `/var/lib/containerd`, and `/usr/local/bin/nerdctl`
mounted so the manager can reach warm workers on `127.0.0.1:<allocatedPort>`.

Dispatch can use authenticated HTTP:

- `GET /container-pools`
- `POST /container-pools/<pool>/warm`
- `POST /container-pools/<pool>/dispatch`

or NATS requests on `dd.remote.container_pool.*.requests`. A NATS request may include `poolSlug` or
`poolId`; otherwise the service matches the message subject to the pool's configured
`nats_subject`. Replies use the NATS reply inbox when present and otherwise publish to
`dd.remote.container_pool.results`.

The generic Postgres config contract is the shared `app_config` block in
`remote/libs/pg-defs/schema/schema.sql` (the single source of truth for every shared
table). The default runtime pool seed is
`remote/databases/pg/seeds/container-pool-app-config.sql`. The seed points at multi-stage runtime
images under `remote/container-pool-rs/runtime-images` for `nodejs`, `rust`, `golang`, `python3`,
`dart`, `gleamlang`, and `erlang`. Dispatch requests never supply a shell command; image, command,
env, request path, warm size, timeout, and NATS subject all come from trusted database config.

## Build server

`dd-build-server` is a Rust CI/CD control surface for cluster-local builds. It accepts authenticated
`POST /builds` requests, clones a repo, builds an image with `nerdctl -n k8s.io build`, and can
apply either a manifest path or kustomize overlay with `kubectl`. Jobs are queued in-process with
`BUILD_SERVER_MAX_CONCURRENT_BUILDS=1`; logs are capped at `BUILD_SERVER_MAX_LOG_BYTES=4194304`
under `/var/lib/dd-build-server/jobs`.

Deploys are intentionally constrained: the request can only choose `deploy.kind` values
`kustomize`, `manifest`, or `none`, paths must stay inside the cloned repo, and target namespaces
must be listed in `BUILD_SERVER_ALLOWED_NAMESPACES` (`default` in the Argo runtime manifest).
Images must include an explicit tag or digest and must match `BUILD_SERVER_ALLOWED_IMAGE_PREFIXES`
(`710156900967.dkr.ecr.us-east-1.amazonaws.com/` in the Argo runtime manifest). ECR pushes are
enabled with `BUILD_SERVER_PUSH_ENABLED=true` and `BUILD_SERVER_ECR_LOGIN_ENABLED=true`; the service
uses env-provided AWS credentials from `dd-agent-secrets` to request an ECR authorization token,
then runs `nerdctl login --password-stdin` before `nerdctl push`.

The deployment mounts the EC2 containerd socket, `/usr/local/bin/nerdctl`, `/usr/bin/kubectl`, and
uses the `dd-build-server` ServiceAccount with a namespace-scoped deployer Role. RBAC is deliberately
narrow: no write access to Secrets, Pods, ServiceAccounts, Jobs, DaemonSets, StatefulSets, or
NetworkPolicies. It can still create Deployments, so treat repo deploy manifests as trusted code;
untrusted repos need a separate empty namespace plus admission controls that block secret mounts,
hostPath, privileged pods, and service-account token automounting.

## Lambda function runner

`/lambdas/functions` is served by the Rust web UI and uses the Rust REST API for CRUD against the
declarative `lambda_functions` table. The REST API publishes change notifications on
`dd.remote.lambdas.functions` so other runtime services can refresh read models or react to state
changes.

Invocation is intentionally not proxied through the REST API. The gateway routes
`POST /lambdas/invoke/<function-id>` directly to `dd-gleam-lambda-runner`, which looks up the active
function definition by UUID, maps the UUID to a reusable worker actor/child process, forwards the
request payload to that process, and exposes child process counters through `/metrics` for
Prometheus/Grafana.

The runtime also includes `dd-mdp-optimizer`, a Rust MDP/POMDP/RL optimization service. It serves
`/mdp/healthz`, `/mdp/metrics`, `POST /mdp/optimize`, and `POST /mdp/telemetry/learn`. It
queue-subscribes to `dd.remote.mdp.optimize` for explicit optimization jobs and
`dd.remote.telemetry.mdp` for app/infra telemetry snapshots, then publishes results to
`dd.remote.mdp.results` plus compact runtime events on `dd.remote.events`.

## Solana contract service

`dd-contract-service` runs `remote/contract-service-rs` as a Rust Solana contract gateway. It serves
`/contracts/healthz`, `/contracts/metrics`, `/contracts/status`, `/contracts/schema`,
`/contracts/example`, `POST /contracts/validate`, `POST /contracts/simulate`, and
`POST /contracts/send`. The service validates `solana.contract.v1` instruction envelopes, checks
base58 public keys and bounded instruction data, can call Solana JSON-RPC `simulateTransaction`
through `SOLANA_RPC_URL`, and blocks `sendTransaction` unless `SOLANA_SEND_ENABLED=true` plus
`CONTRACT_SEND_AUTH_SECRET` are configured. Request `cluster` must match the configured
`SOLANA_CLUSTER`, private RPC URLs require `SOLANA_ALLOW_PRIVATE_RPC=true`, and `skipPreflight`
requires `SOLANA_ALLOW_SKIP_PREFLIGHT=true`.

The deployment queue-subscribes to `dd.remote.contracts.solana.validate` with queue group
`dd-contract-service`, publishes validation results to `dd.remote.contracts.solana.results`, and
emits compact lifecycle events on `dd.remote.events`.

## AI/ML feature pipeline

`dd-ai-ml-pipeline` runs `remote/ai-ml-pipeline` as a long-lived Python3 service in the `ai-ml`
namespace. It accepts raw telemetry through `POST /ml/analyze`, `POST /ml/ingest`, or the
`dd.remote.telemetry.raw` NATS subject. The online model turns metrics into normalized features,
EWMA baselines, z-score anomaly scores, state/risk summaries, action-impact hints, and transition
estimates. Gateway traffic forwards the internal `X-Server-Auth` header, and the service requires
the mirrored `SERVER_AUTH_SECRET` in the `ai-ml` namespace for all non-probe HTTP routes.

When traffic uses `POST /ml/ingest` or NATS input, the service publishes:

- feature events to `dd.remote.ml.features`
- MDP-ready telemetry to `dd.remote.telemetry.mdp`
- compact runtime events to `dd.remote.events`
- rejected NATS message summaries to `dd.remote.ml.deadletter`

The heavier open-source platform choices live in `remote/argocd/ai-ml-platform` and the matching
Argo CD application manifests: Dagster, Airflow, MLflow, dbt, Kafka through Strimzi, Spark,
Metaflow, LlamaIndex, Qdrant, and Airbyte.

## Web scraper service

`dd-web-scraper` runs `remote/web-scraper-service` as a long-lived Node.js/Fastify service from the
Playwright browser image. It exposes `GET /healthz`, `GET /metrics`, `GET /strategies`, and
`POST /scrape`; the gateway mirrors those as `/scrape/healthz`, `/scrape/metrics`,
`/scrape/strategies`, and `POST /scrape`.

The strategy list is `native-fetch`, `cheerio`, `jsdom`, `linkedom`, `playwright`, `puppeteer`, and
`browserless`. `auto` chooses Playwright for JavaScript rendering, Cheerio for selector extraction,
and native fetch for plain requests. Browserless stays opt-in through `BROWSERLESS_TOKEN`, and
private or cluster-local targets are blocked unless `SCRAPER_ALLOW_PRIVATE_NETWORKS=true`.

HTML extraction runs in Node `worker_threads` through `src/extraction-worker.ts`; the pod sets
`SCRAPER_PARSER_WORKERS=2` and `SCRAPER_PARSER_WORKER_MEMORY_MB=128` so parser CPU and memory use
do not block Fastify or browser orchestration. The service fails closed when `SERVER_AUTH_SECRET`
is missing, revalidates redirect and browser subresource targets, and blocks URL credentials plus
sensitive outbound headers unless their explicit opt-in env vars are enabled.
