# `remote/argocd/dd-next-runtime`

GitOps manifests for the baseline runtime that should always be visible in Argo:

- `dd-remote-web-home` (Rust web layer for `/` + `/home`)
- `dd-remote-auth` (Rust PIN auth service that sets the gateway `dd_auth` cookie)
- `dd-remote-rest-api` (Rust RDS/Postgres REST API for agent task data)
- `dd-agent-worker-broker` (Rust worker-dispatch broker for NATS-first, direct-if-awake handoff)
- `dd-gleam-lambda-runner` (Gleam child-process runner for user-defined lambda invocations)
- `dd-remote-queue-consumer` (Rust NATS shadow consumer that prepares UUID-bound thread workers)
- `dd-webrtc-signaling` (Rust WebRTC room signaling over WebSocket)
- `dd-web-scraper` (Node.js/Fastify scraping worker with browser and DOM strategies)
- `dd-dev-server-api` (bootstrap Node.js coding-agent task manager for `/tasks`, `/stream`,
  `/status`, `/agents`, `/healthz`)
- `dd-remote-gateway` (public k8s path splitter on host ports 80 and 443)
- `dd-idle-reaper` (Rust scheduler for idle sweeps and the 90-minute cluster doctor task)

Gateway path map:

- `/`, `/home` -> `dd-remote-web-home:8080`
- `/auth` -> `dd-remote-auth:8083`
- `/agents/tasks` -> `dd-remote-web-home:8080`
- `/api/agents/tasks` -> `dd-remote-rest-api:8082`
- `/api/agents/threads/<uuid>/prepare` -> `dd-remote-rest-api:8082` (internal auth required)
- `POST /api/agent-worker/threads/<uuid>/tasks` -> `dd-agent-worker-broker:8098`
- `/lambdas/functions` -> `dd-remote-web-home:8080`
- `/api/lambdas/functions` -> `dd-remote-rest-api:8082`
- `POST /lambdas/invoke/<function-id>` -> `dd-gleam-lambda-runner:8083` directly
- `/webrtc/`, `/webrtc/healthz`, `/webrtc/metrics`, `/webrtc/signal` -> `dd-webrtc-signaling:8095`
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
`dd-remote-gateway-tls` in the `default` namespace. HTTP remains enabled for bootstrap access and
for ACME HTTP-01 challenge renewal.

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
- model-provider keys like `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`, and `OPENAI_API_KEY`
- GitHub credentials used by the remote dev worker entrypoint and PR creation path

Use `dd-remote-rest-api-secrets` for RDS-specific values:

- `AGENT_TASKS_RDS_DATABASE_URL`
- `RDS_DATABASE_URL`

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

This proves the queue handoff and warmup behavior without allowing generic workers to steal
thread-affine execution. The Node.js worker still executes the task through the direct REST handoff
until the queue path is promoted from shadow mode.

`dd-agent-worker-broker` is the additive long-run replacement for REST-owned worker dispatch. Its
first route, `POST /api/agent-worker/threads/<threadId>/tasks`, publishes the task to JetStream,
emits the wakeup subject, direct-posts to the deterministic Node.js worker only when that worker is
already healthy, and otherwise scales the worker Deployment to `1` while returning `202 queued`.

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
