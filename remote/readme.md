# `remote/` — long-running services that live outside Vercel

This directory holds code that **cannot run on Vercel** because it needs something Vercel's
serverless model doesn't give us: a persistent filesystem, a long-lived process, the ability to
spawn child processes that outlive an HTTP request, or all three.

Today there are several key runtime services:

- [`dev-server/`](./dev-server/) — Node worker/API runtime for task dispatch, streaming, and agent
  execution
- [`web-home-rs/`](./web-home-rs/) — Rust public web layer for `/` + `/home`
- [`rest-api-rs/`](./rest-api-rs/) — Rust REST API boundary for RDS/Postgres-backed agent task data
- [`build-server-rs/`](./build-server-rs/) — Rust CI/CD server for authenticated repo image builds
  and controlled Kubernetes deploys
- [`contract-service-rs/`](./contract-service-rs/) — Rust Solana contract gateway for validated
  instruction envelopes, signed transaction simulation, and NATS validation results.
- [`ai-ml-pipeline/`](./ai-ml-pipeline/) — Python3 online feature pipeline for telemetry analysis
  and MDP-ready AI/ML signals.
- [`trading-server-rs/`](./trading-server-rs/) — Rust trading decision service that turns scraper,
  AI/ML, market, and MDP/POMDP inputs into risk-gated NATS order intents. Broker metadata is seeded
  through [`databases/pg/seeds/trading-platform-app-config.sql`](./databases/pg/seeds/trading-platform-app-config.sql).

Future entries (a long-running queue worker, a stateful LLM evaluator, a headless browser farm,
etc.) would live as siblings.

There are also runtime siblings for queueing, scheduling, and optimization:

- [`queue-consumer-rs/`](./queue-consumer-rs/) — Rust NATS shadow consumer that prepares UUID-bound
  thread workers and de-dupes taskIds.
- [`idle-reaper-rs/`](./idle-reaper-rs/) — Rust maintenance supervisor for idle sweeping, cluster
  doctor prompts, NATS watchdog work, and the daily 4am Eastern worker-image build.
- [`mdp-optimizer-rs/`](./mdp-optimizer-rs/) — Rust MDP/POMDP/RL optimizer that consumes NATS jobs
  and publishes optimization results.

The AI/ML platform seed layer lives in
[`argocd/ai-ml-platform/`](./argocd/ai-ml-platform/) and is managed by the
`dd-ai-ml-platform` ArgoCD Application. It installs the Python feature pipeline and a cluster
catalog for Dagster, Airflow, MLflow, dbt, Kafka, Spark, Metaflow, LlamaIndex, Qdrant, and Airbyte.
The heavier tools have separate Argo CD Application manifests in [`argocd/apps/`](./argocd/apps/).

The baseline Kubernetes runtime bundle lives in
[`argocd/dd-next-runtime/`](./argocd/dd-next-runtime/) and is managed by the
`dd-next-runtime` ArgoCD Application. Apply
[`argocd/apps/dd-next-runtime.application.yaml`](./argocd/apps/dd-next-runtime.application.yaml)
when bootstrapping Argo, then let Git + Argo own runtime Deployment, Service, ConfigMap, and gateway
changes.

There are also two Gleam/OTP services with their own ArgoCD Application manifests for both Minikube
and EC2 k8s paths:

- [`gleamlang-server/`](./gleamlang-server/) — WebSocket streaming service.
- [`gleam-mcp-server/`](./gleam-mcp-server/) — MCP JSON-RPC service with read-only runtime tools
  and Prometheus metrics.

The cluster observability stack lives in [`argocd/observability/`](./argocd/observability/) and is
managed by the `dd-observability` ArgoCD Application. It installs Prometheus, Grafana,
OpenTelemetry Collector, Loki, Promtail, Tempo, and Jaeger as separate deployments/daemonsets.

The cluster messaging layer lives in [`argocd/messaging/`](./argocd/messaging/) and is managed by
the `dd-messaging` ArgoCD Application. It runs NATS + JetStream and a Prometheus exporter sidecar.

The cluster VPN and bastion layer lives in [`argocd/vpn/`](./argocd/vpn/) and is managed by the
`dd-vpn` ArgoCD Application. It runs WireGuard through wg-easy and a Rust `dd-bastion` access
broker that serves VPN/cluster profiles plus a read-only kubeconfig over the VPN.

The cluster secret-management layer lives in [`argocd/secrets/`](./argocd/secrets/) and is managed
by the `dd-secrets` ArgoCD Application after `external-secrets-operator` is installed. GitHub
stores only SecretStore/ExternalSecret manifests; AWS Secrets Manager stores the real values.

## Why a separate `remote/` at all

Vercel functions max out at **800s** (Fluid Compute), have no persistent filesystem, and can't run
a daemon. A fresh `git clone` of `dd-next-1` plus `pnpm install` takes 2–5 minutes — unacceptable
as a per-request cost. We need a place where the working tree, `node_modules`, the pnpm store, and
`.next/` cache **persist between prompts in the same thread**.

That place is a Docker container running somewhere it can stay up: a small VM, an ECS/Fargate task,
or a Fly Machine. The production model is one thread UUID → one runtime container/workspace/branch;
subsequent prompts in that thread reuse the same runtime. Starting a new container for every raw
HTTP request is intentionally avoided because it would lose conversation/workspace continuity and
make reconnects expensive.

The thinking behind this split is documented in:

- [`../docs/dev-hybrid-chat-plan.md`](../docs/dev-hybrid-chat-plan.md) (v1 — the rejected
  pure-Vercel design and why it doesn't work for this repo)
- [`../docs/dev-hybrid-chat-plan-v2.md`](../docs/dev-hybrid-chat-plan-v2.md) (v2 — long-lived VM
  workspace)
- [`../docs/dev-hybrid-chat-plan-v3.md`](../docs/dev-hybrid-chat-plan-v3.md) (v3 — Fargate per
  task; superseded by v4)
- **[`../docs/dev-hybrid-chat-plan-v4-k8s.md`](../docs/dev-hybrid-chat-plan-v4-k8s.md) (v4 —
  current target: bare-EC2 Kubernetes, one pod per thread)**

## Where things run (target + current EC2 status)

- Cluster: vanilla Kubernetes on plain EC2 (kubeadm or k3s — your call, manifests work
  identically). **No EKS, no ECS, no Fargate.**
- Pod model: **one Pod per thread/conversation**, identified by the thread UUID created at
  `/u/admin/remote-dev`. New threads → fresh pods spawn on demand. Existing threads → routed back
  to their pod by the Kubernetes per-thread Ingress path `/dd-thread/<short>/...`; Redis and the
  k8s API are control-plane lookup/provisioning aids, not Node.js routing.
- Sleep / wake: a control-plane idle reaper scales per-thread Deployments to `replicas=0` when no
  active task exists and thread activity is older than `REMOTE_DEV_IDLE_TIMEOUT_MS`. The PVC stays.
  Next dispatch on that threadId scales back to `replicas=1` and the workspace remounts intact.
- End thread: deletes the Ingress + Deployment + Service + PVC for that pod. GitHub branch + PR
  survive.
- Image cadence: every 3 days at 04:00 ET, GitHub Actions rebuilds `dd-dev-server:latest` and
  pushes to ECR so a freshly-spawned pod's `git fetch + checkout --hard origin/dev + pnpm install`
  has minimal delta to apply.

The k8s manifests for all of this are in [`./k8s/`](./k8s/) — see
[`./k8s/readme.md`](./k8s/readme.md) for the cluster bring-up walkthrough and the Vercel ↔
kube-apiserver wiring.

### Current runtime snapshot (verified May 14, 2026)

The target above is the long-term shape. During bootstrap hardening, we also run a stable
host-level service deployment so `/` and `/home` stay reachable while per-thread automation is
being tuned:

- Host verified in active use: `http://54.91.17.58/` and `http://54.91.17.58/home`
- HTTPS is also terminated at the gateway with a self-signed certificate: `https://54.91.17.58/`
  and `https://54.91.17.58/home`
- K8s runtime entrypoint: Deployment `dd-remote-gateway` in namespace `default`, with
  `hostPort: 80` and `hostPort: 443`
- Public web deployment behind the gateway: `dd-remote-web-home`
- Internal/public JSON API deployment behind the gateway: `dd-remote-rest-api`
- Worker dispatch broker behind the gateway: `dd-agent-worker-broker`
- Authenticated build/deploy server behind the gateway: `dd-build-server`
- Bootstrap Node.js coding-agent task manager behind the gateway: `dd-dev-server-api`
- Public telemetry UI behind the gateway: `http://54.91.17.58/telemetry/` (Grafana, requires the
  configured dd gateway auth header)
- Public ops paths behind the gateway:
  - `https://54.91.17.58/agents/tasks` (Rust diagnostics task/thread/PR table, public during
    bootstrap)
  - `https://54.91.17.58/agents/threads` (Rust thread-first chat UI with stored response stream and
    feedback)
  - `https://54.91.17.58/api/agents/tasks` (Rust REST API snapshot, public during bootstrap)
  - `https://54.91.17.58/api/agent-worker/threads/<threadId>/tasks` (Rust worker broker, requires
    `Auth`)
  - `https://54.91.17.58/container-pools` (Rust container pool control surface, requires `Auth`)
  - `https://54.91.17.58/bastion/runtime/deployments` (Rust `dd-bastion` access broker inventory,
    requires `Auth`; browser terminal routes are disabled by default)
  - `https://54.91.17.58/builds` (Rust build server, requires `Auth`)
  - `http://54.91.17.58/prometheus/` (Prometheus, requires `Auth`)
  - `http://54.91.17.58/nats/` (NATS monitor, requires `Auth`)
  - `http://54.91.17.58/nats-metrics/metrics` (NATS exporter, requires `Auth`)
  - `http://54.91.17.58/gleam/home` and `wss://54.91.17.58/gleam/ws` (Gleam WebSocket service,
    requires `Auth`)
  - `http://54.91.17.58/mcp`, `http://54.91.17.58/mcp/home`, and `http://54.91.17.58/mcp/metrics`
    (Gleam MCP service, requires `Auth`)
  - `http://54.91.17.58/reaper/` and `http://54.91.17.58/cron/` (runtime service status surfaces,
    require `Auth`)
- `curl http://127.0.0.1/` -> `302` redirect to `/home`
- `curl http://127.0.0.1/home` -> HTML controls page returned

This fallback service keeps the box observable while we continue promoting the full per-thread pod
path in `dd-dev`.

`/agents/tasks` and `/agents/threads` are served by the Rust web deployment, not Vercel/Next.js.
They are HTML-only; the browser calls the public gateway routes `/api/agents/tasks` and
`/api/agents/tasks/:taskId/events` directly. The REST API owns RDS/Postgres access via
`AGENT_TASKS_RDS_DATABASE_URL` or `RDS_DATABASE_URL`, with `AGENT_TASKS_DATABASE_URL` /
`DATABASE_URL` and Supabase REST as migration fallbacks. When we deploy Postgres inside the
cluster, only the REST API needs to point at that internal service.

`dd-agent-worker-broker` is the intended long-run home for Node.js worker lifecycle dispatch. The
REST API still owns the existing `/api/agents/threads/:threadId/tasks` path during migration, while
the broker exposes `/api/agent-worker/threads/:threadId/tasks` for NATS-first dispatch, wakeup, and
direct-if-awake handoff to the pinned worker. Until the UI is moved over, it is acceptable for the
REST API to keep brokering worker calls; after the broker path is proven, the REST API should shed
worker wake/dispatch/stream responsibilities and stay focused on data/API ownership.

Temporary gateway auth is intentionally simple during bootstrap: protected ops paths only pass when
the request includes the configured dd gateway auth header. Public responses must not reveal that
header value. The task UI/API routes above remain public bootstrap surfaces. Longer-term, replace
the static header with TLS plus an identity-aware gateway (`auth_request` with oauth2-proxy,
Cloudflare Access, or Tailscale), keep worker/NATS/control-plane services internal, and add
NetworkPolicies plus least-privilege service accounts.

## Secrets And Key Rotation

Do not put raw model keys, GitHub tokens, database URLs, or gateway secrets in GitHub. The intended
flow is:

1. Install `external-secrets-operator` from
   [`argocd/apps/external-secrets-operator.application.yaml`](./argocd/apps/external-secrets-operator.application.yaml).
2. Store real values in AWS Secrets Manager under:
   - `dd/remote-dev/agent-secrets`
   - `dd/remote-dev/rest-api-secrets`
   - `dd/remote-dev/idle-reaper-secret`
3. Sync [`argocd/secrets/`](./argocd/secrets/) with ArgoCD. External Secrets Operator creates the
   Kubernetes secrets `dd-agent-secrets`, `dd-remote-rest-api-secrets`, and
   `dd-idle-reaper-secret`.
4. Deployments consume those Kubernetes secrets with `envFrom` or `secretKeyRef`; worker pods
   inherit model-provider keys through `dd-agent-secrets`. `dd-build-server` also reads optional
   `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, and `AWS_SESSION_TOKEN` keys from
   `dd-agent-secrets` when it needs to push images to ECR.

For now, updates can be done from AWS Console or CLI by editing the AWS Secrets Manager JSON. A
good admin UI would be a small authenticated page that calls a server-side AWS SDK endpoint to
update one named secret version, never returning the existing secret value to the browser. A GitHub
Actions alternative is a manual `workflow_dispatch` with masked environment inputs that writes to
AWS Secrets Manager and then runs `argocd app sync dd-secrets`; that keeps Git history clean while
still giving an audit trail.

When a model or provider key changes, update the AWS secret, let External Secrets refresh, then
restart the affected deployments so env vars reload. Long term, prefer an EC2 instance profile or
IRSA-style identity for External Secrets instead of static AWS access keys.

Identity note: app workloads should keep consuming Kubernetes secrets generated by External Secrets,
not call AWS Secrets Manager directly. The EC2 host can have an instance role for cluster and ECR
operations, but direct Secrets Manager reads for `dd/remote-dev/*` are not required on the host and
may be denied. When replacing `dd-aws-secrets-manager-auth`, grant only
`secretsmanager:GetSecretValue` and `secretsmanager:DescribeSecret` on
`arn:aws:secretsmanager:us-east-1:710156900967:secret:dd/remote-dev/*` to a dedicated External
Secrets identity, ideally IRSA on EKS or the equivalent least-privilege principal for the current
self-managed EC2 path. If the EC2 instance profile is used for this, first make sure pods cannot
reach node IMDS credentials broadly; otherwise any compromised pod could inherit node-level
secret-read access.

### Managing Secrets

AWS Secrets Manager is the source of truth for live credentials. GitHub should contain only:

- ArgoCD `Application` manifests.
- External Secrets Operator `ClusterSecretStore` / `ExternalSecret` manifests.
- Documentation that names expected keys, never secret values.

The current secret groups are:

| AWS secret                         | Kubernetes secret             | Consumers                                                                         |
| ---------------------------------- | ----------------------------- | --------------------------------------------------------------------------------- |
| `dd/remote-dev/agent-secrets`      | `dd-agent-secrets`            | Node.js coding-agent task workers, warm worker templates, and optional build-server ECR push credentials. |
| `dd/remote-dev/rest-api-secrets`   | `dd-remote-rest-api-secrets`  | Rust REST API / lifecycle service.                                                |
| `dd/remote-dev/idle-reaper-secret` | `dd-idle-reaper-secret`       | Rust idle reaper and cron service.                                                |
| `dd/remote-dev/mcp-secrets`        | `dd-gleam-mcp-server-secrets` | Future write-capable MCP tools only; the first MCP service should stay read-only. |

Safe update flow:

1. Write a new version of the AWS Secrets Manager JSON secret.
2. Sync the `dd-secrets` ArgoCD application, or wait for External Secrets Operator to refresh.
3. Restart only deployments that read those env vars, because env values are loaded at process
   start.
4. Verify with health endpoints and telemetry. Do not log, print, or return secret values while
   testing.

Any key pasted into chat, logs, PR comments, or a terminal transcript should be treated as exposed
and rotated. The replacement key can keep the same env-var name; the AWS secret version changes,
then the deployment restart picks it up.

### Admin UI Option

A cluster-served admin page such as `/agents/secrets` can make rotation comfortable without making
Git dangerous. It should:

- Require real admin auth before it is enabled publicly.
- List secret groups, expected env-var names, last sync time, ExternalSecret status, and which
  deployments consume each group.
- Never show existing values, even to admins.
- Accept one-way replacement values and send them to a server-side route that calls AWS Secrets
  Manager.
- Store an audit event with secret group, key name, AWS version id, actor, and affected
  deployments.
- Offer a `sync + restart` action that runs ArgoCD sync for `dd-secrets` and restarts only the
  selected deployments.

This UI can live in the Rust web server for cluster-local operations, or in the Next.js admin area
if we want it beside `/u/admin/remote-dev`. In either shape, the browser talks to our server, and
only the server uses AWS credentials.

### GitHub Actions Option

For now, a manual GitHub Actions `workflow_dispatch` is the cleanest auditable automation path:

1. The workflow accepts a secret group enum and key name.
2. The new value is supplied as a masked environment input or through an approved GitHub
   Environment.
3. GitHub uses AWS OIDC to assume a narrow role that can update only `dd/remote-dev/*` secrets.
4. The workflow calls AWS Secrets Manager to create a new secret version.
5. It triggers `argocd app sync dd-secrets`.
6. It restarts the deployments listed in a repo manifest, then posts a redacted summary to the run.

Do not use GitHub webhooks to transmit raw secret values. Hooks can trigger syncs when manifests
change, but the actual values should move directly from an authenticated admin path or GitHub
Actions runtime to AWS Secrets Manager.

Gateway HTTPS uses the Kubernetes TLS secret `dd-remote-gateway-tls` in the `default` namespace.
The gateway also serves ACME HTTP-01 challenge files from `/home/ec2-user/dd-acme-webroot` at
`/.well-known/acme-challenge/`, which lets Certbot 5.4+ request a trusted Let's Encrypt IP-address
certificate for `54.91.17.58` in webroot mode. Let's Encrypt IP-address certificates use the
short-lived profile, so renewal must rewrite the Kubernetes secret and restart `dd-remote-gateway`.

### Live cluster dashboard — `/u/admin/k8s`

A rich admin-only dashboard for the cluster lives at
[`/u/admin/k8s`](<../src/app/(pages)/(private)/u/admin/k8s/page.tsx>). Built directly on the same
`K8S_API_SERVER` / `K8S_NAMESPACE` / `K8S_SA_TOKEN` env contract the dispatch path uses, so when
the control plane works, the dashboard works.

What it shows:

- **Counters** for each lifecycle state — `active` (Running+Ready), `starting` (Pending),
  `sleeping` (Deployment scaled to 0, PVC kept), `failed` (CrashLoopBackOff / restartCount>5),
  `dead` (no Pod, no obvious reason — cleanup candidate).
- **Per-thread table:** name, derived state, threadId prefix, ready/desired replicas, podIP+node,
  image (digest), age, plus per-row actions:
  - `Sleep` → `kubectl scale ... --replicas=0` (PVC retained)
  - `Wake` → `kubectl scale ... --replicas=1` (workspace remounts)
  - `Delete` → tears down Ingress + Deployment + Service + PVC (irreversible)
- **Cluster nodes** — Ready/NotReady, allocatable + capacity for cpu/mem/pods.
- **Recent k8s events** in the `dd-dev` namespace — useful for debugging "why won't this pod start"
  without shelling into the cluster.

The client uses RxJS heavily: independent polling streams per endpoint (5s overview / 10s pods /
15s events / 60s nodes), unified visibility gate that pauses polling on hidden tabs,
exponential-backoff retry, `shareReplay` multicasting, and a `Subject<{target,kind}>` action stream
that fans into a `switchMap` fetch pipeline so concurrent sleep/wake/delete clicks are queued and
tracked through one source of truth. See
[`k8s-client.tsx`](<../src/app/(pages)/(private)/u/admin/k8s/k8s-client.tsx>).

Backing API routes (admin-auth'd, all under `/api/admin/k8s/`):

- `GET /overview` — cluster counts
- `GET /pods` — joined Deployment+Pod list with derived state
- `GET /events?limit=…` — recent namespace events
- `GET /nodes` — node capacity/allocatable
- `POST /pods/<name>/sleep` — scale to 0
- `POST /pods/<name>/wake` — scale to 1
- `DELETE /pods/<name>` — full teardown

## Source layout under `remote/`

```
remote/
├── readme.md          # this file
├── dev-server/        # worker/API runtime (Node TS server)
│   ├── readme.md      # build/run/env-var reference
│   └── src/           # server.ts, agents/*, storage/*, realtime.ts, …
├── web-home-rs/       # public homepage server (/ + /home)
│   ├── Cargo.toml
│   ├── readme.md
│   └── src/
│       └── main.rs
├── rest-api-rs/       # Rust REST API boundary for RDS/Postgres agent data
│   ├── Cargo.toml
│   ├── readme.md
│   └── src/
│       └── main.rs
├── argocd/
│   ├── messaging/     # NATS + JetStream + prometheus exporter
│   └── observability/ # Prometheus/Grafana/OTel/Loki/Tempo/Jaeger
├── ec2/               # stock Amazon Linux 2023 bootstrap for plain EC2 hosts
│   ├── README.md
│   └── bootstrap-amazon-linux-2023-k8s.sh
├── ami/               # Packer AMI definition for the single-node k8s EC2 host
│   ├── README.md      # AMI build walkthrough
│   ├── k8s-dev-node.pkr.hcl
│   ├── bootstrap-cluster.sh
│   └── scripts/      # 01-base-system through 08-cleanup provisioners
├── k8s/               # vanilla Kubernetes manifests for v4 (single-node)
│   ├── readme.md      # cluster bring-up walkthrough
│   ├── 00-namespace.yaml
│   ├── 01-configmap.yaml
│   ├── 02-secrets.template.yaml
│   ├── 03-rbac.yaml
│   ├── 04-network-policy.yaml
│   ├── 05-resource-quota.yaml
│   ├── 06-thread-pvc.template.yaml
│   ├── 07-thread-deployment.template.yaml
│   ├── 08-thread-service.template.yaml
│   └── 09-thread-ingress.template.yaml
├── ws-loadtest-rs/    # rust websocket load generator (5k clients)
└── gleamlang-ws-loadtest/ # gleam websocket load generator (5k clients)
```

## Observability and APM

The EC2 k8s cluster has a separate observability plane:

- `dd-otel-collector` receives explicit OTLP traces from the Node worker and scrapes Prometheus
  metrics from the Node, Rust, and Gleam runtimes.
- `dd-prometheus` stores collector-exported metrics and is exposed through the public gateway at
  `/prometheus/`.
- `dd-grafana` serves the HTML dashboard at `/telemetry/`.
- `dd-loki` + `dd-promtail` collect container logs from `/var/log/containers`.
- `dd-tempo` and `dd-jaeger` receive traces from the collector.
- `dd-nats` exposes exporter metrics on `dd-nats.messaging.svc.cluster.local:7777`, and the
  collector scrapes it. The public gateway also exposes `/nats/` and `/nats-metrics/metrics`.

Runtime telemetry is deliberately explicit:

- Node `remote/dev-server` does **not** use OpenTelemetry auto-instrumentation or monkey-patching.
  It emits direct OTLP/HTTP spans from local calls in `src/telemetry.ts` and exposes `/metrics`.
- Rust `remote/web-home-rs` uses Prometheus counters/gauges and exposes `/metrics`.
- Rust `remote/rest-api-rs` uses Prometheus counters/gauges and exposes `/metrics`; the
  OpenTelemetry Collector scrapes it as `dd-remote-rest-api`.
- Gleam `remote/gleamlang-server` reports actor-backed WebSocket connection, tick, HTTP, and
  message counters at `/metrics`.
- Gleam `remote/gleam-mcp-server` reports HTTP and JSON-RPC method counters at `/metrics`; the
  OpenTelemetry Collector scrapes it as `dd-gleam-mcp-server`.

Grafana starts with the provisioned dashboard `Remote Dev Runtime Overview`, which tracks request
rates, Node worker events, NATS connections/throughput/resources, the Gleam MCP runtime, and the
10k WebSocket load-test connection count.

## Messaging

NATS is available in-cluster at:

- client URL: `nats://dd-nats.messaging.svc.cluster.local:4222`
- monitoring URL: `http://dd-nats.messaging.svc.cluster.local:8222`
- metrics URL: `http://dd-nats.messaging.svc.cluster.local:7777/metrics`

JetStream is enabled and stores data on the EC2 node at `/var/lib/dd/nats`. That is appropriate for
the current single-node EC2 cluster; move it to a proper storage class before running multiple
worker nodes.

## What `dev-server/` is

A Node.js + TypeScript HTTP server (Fastify) that:

1. Receives a task from Vercel: `POST /tasks { taskId, threadId, prompt }`.
2. Creates or reuses the thread workspace/branch `agent/k8s/openai-5.5/<threadId>/<slugified-thread-title>`
   from the warm baseline of `dd-next-1` on `dev`.
3. Runs the selected provider (`openai-sdk` default, Claude SDK/CLI, or OpenAI Codex
   CLI) inside the thread workspace.
4. Streams every event:
   - back to Vercel via `POST /api/admin/remote-dev/events` (server-to-server, used to populate
     NeonDB), **and**
   - to any browser SSE subscriber on `GET /stream/:taskId`.
5. After the agent finishes:
   - opens or reuses a PR against `dev` via `gh pr view/create`,
   - scans `outputs/<taskId>/` and uploads any files via the configured storage adapter (S3 / R2 /
     GCS / Google Drive / local), emitting an `artifact` event per file with the resulting URL,
   - emits a terminal `done` event.
6. Exposes worker APIs (`/tasks`, `/stream/:taskId`, `/status`, `/agents`, `/healthz`) behind the
   cluster routing layer.

The container keeps only hot runtime state. NeonDB remains the source of truth for
threads/tasks/events/artifacts; the thread branch and PR let a restarted container resume from
GitHub when needed.

See [`dev-server/readme.md`](./dev-server/readme.md) for build, run, and the full env-var
reference.

## Control plane vs worker plane

The repo already splits remote-dev into two cooperating pieces:

1. **Control plane**: the Vercel/Next.js app plus
   [`src/lib/server/remote-dev/container-registry.ts`](../src/lib/server/remote-dev/container-registry.ts)
   and [`docker-client.ts`](../src/lib/server/remote-dev/docker-client.ts). This side serves the
   admin UI, tracks thread UUIDs, resolves a UUID to a live pod through Redis first and the
   Kubernetes API second, and wakes a sleeping pod by scaling its Deployment from `replicas: 0` to
   `1`.
2. **Worker plane**: one `remote/dev-server/` runtime per thread. That container fetches
   `origin/dev`, accepts new commands, updates PRs, and runs tests. Public `/` and `/home` now come
   from `remote/web-home-rs`, not the worker runtime.

That split is why the cluster can safely sleep idle pods and still bring back the matching
UUID-bound worker when the next request arrives. Kubernetes Ingress/Service selection owns UUID
routing; the Node.js process inside a worker only runs taskIds for its pinned thread and rejects
mismatches.

## How it fits with the Vercel app

```
┌─────────────────┐
│  phone browser  │
└────────┬────────┘
         │ HTTPS (Clerk-authed admin)
         ▼
┌─────────────────────────────────────────────────────────┐
│  Next.js app on Vercel  (this repo)                     │
│   • /u/admin/remote-dev — chat UI (React)               │
│   • POST /api/admin/remote-dev/dispatch  → docker       │
│   • POST /api/admin/remote-dev/events    (ingest)       │
│   • GET  /api/admin/remote-dev/stream/:id (SSE)         │
│   • POST /api/admin/remote-dev/sign-token (HMAC token)  │
│   • POST /api/admin/remote-dev/cancel/:id               │
│   • Drizzle/postgres-js → NeonDB                        │
│   • Upstash KV (in-flight task cache)                   │
└──┬───────────────────────────┬─────────────┬────────────┘
   │ POST /tasks               │ ingest      │
   │ (X-Server-Auth)           │ events      │
   │                           ▲             │
   │                           │             │
   ▼                           │             ▼
┌─────────────────────────────────────┐  ┌────────────────┐
│  remote/dev-server/  (Docker)       │  │   NeonDB       │
│  • git worktree per task            │  │ agent_remote_  │
│  • runs OpenAI SDK by default       │  │  dev_threads   │
│  • streams events                   │  │  …_tasks       │
│  • opens PRs                        │  │  …_events      │
│  • publishes outputs to S3/GCS/R2/  │  │  …_artifacts   │
│    Drive                            │  └────────────────┘
└──┬─────────────────────────┬────────┘
   ▼                         ▼
┌────────────┐       ┌──────────────────────┐       ┌──────────────────────┐
│  GitHub    │       │  OpenAI API          │       │  S3 / GCS / R2 /     │
│ (truth +   │       │  (gpt-5.5 default)   │       │  Drive (artifacts)   │
│  PRs)      │       │                      │       │                      │
└────────────┘       └──────────────────────┘       └──────────────────────┘
```

Five communication paths:

| From             | To                                                          | Auth                                        | Used for                                                    |
| ---------------- | ----------------------------------------------------------- | ------------------------------------------- | ----------------------------------------------------------- |
| Vercel API route | Docker `/tasks`, `/cancel`, `/tasks` (snapshot), `/healthz` | `X-Server-Auth` shared secret               | Dispatch + cancel + first-load snapshot                     |
| Docker           | Vercel `/api/admin/remote-dev/events`                       | `X-Agent-Auth` shared secret                | Persist every streamed event into NeonDB                    |
| Docker           | Vercel `/api/admin/remote-dev/heartbeat`                    | `X-Heartbeat-Auth` shared secret            | Periodic in-flight snapshot for the UI's adaptive-poll loop |
| Docker           | Supabase Broadcast `remote-dev:user:<ddUserId>`             | Supabase service-role key                   | Per-user live event fan-out — **lambda-independent**        |
| Browser          | Supabase WebSocket (channel `remote-dev:user:<ddUserId>`)   | Supabase anon key                           | Live subscription — direct, no Vercel in the loop           |
| Browser          | Docker `/stream/:taskId?token=…`                            | HMAC-signed token from Vercel `/sign-token` | Optional fallback when Supabase Realtime is degraded        |

**Why two live paths (Supabase + heartbeat-driven polling):** Supabase Broadcast is the primary and
lowest-latency. The Vercel heartbeat lets the UI display "is docker alive" and back off polling
cadence (20s healthy → 40s degraded). NeonDB is always the durable truth — both paths converge
there via the `/events` ingest route.

Everything that survives a restart lives in NeonDB
(`agent_remote_dev_threads / _tasks / _events / _artifacts`) plus Upstash Redis as a hot cache for
the in-flight set. The container itself only needs the credentials in `process.env`.

### Worker DB contract (no Drizzle in the container)

The worker container does **not** use Drizzle or direct SQL connections to NeonDB. Instead:

- durable task/event/task-status writes flow through `EVENT_INGEST_URL`
  (`/api/admin/remote-dev/events`) on the Next.js server
- live fan-out goes through Supabase Broadcast channels (`remote-dev:user:<ddUserId>`) using the
  Supabase service role key

This keeps DB schema ownership on the Next.js side while allowing workers to communicate over
authenticated HTTP/webhook calls and Supabase client APIs.

## UUID routing, branches, and PR links

- The frontend creates or reuses a thread UUID.
- Dispatch always goes through the Next.js API first: `POST /api/admin/remote-dev/dispatch`.
- Next.js resolves thread -> worker container/pod and forwards to `POST /tasks` on the worker with
  `X-Server-Auth`.
- Reusing the same UUID reuses the same runtime session and branch.
- Branch naming convention is now: `agent/k8s/openai-5.5/<threadId>/<title-and-explanation-slug>`.
- Completed runs emit a `done` event with `prUrl`; the admin pages surface those links in both
  `/u/admin/remote-dev` and `/u/admin/remote-ui`.

Recent host-side smoke test (same UUID reused on two task IDs):

- thread UUID: `00000000-0000-4000-8000-000000000001`
- task IDs: `11111111-1111-4111-8111-111111111111`, `22222222-2222-4222-8222-222222222222`
- both accepted and routed to the same branch/session path

## Next.js stream path and duration

The chat UI should always call Next.js first. Current server routes:

- `POST /api/admin/remote-dev/dispatch` (enqueue + forward to worker)
- `GET /api/admin/remote-dev/stream/:taskId` (SSE relay/replay)
- `POST /api/admin/remote-dev/sign-token` (direct worker SSE fallback token)
- `POST /api/admin/remote-dev/reaper/sweep` (authenticated idle sweep; scales idle thread
  Deployments to `replicas=0`)

`/api/admin/remote-dev/stream/:taskId` currently caps a single open stream at 12 minutes
(`MAX_STREAM_MS = 12 * 60 * 1000`), which comfortably covers the 300-second open-stream
requirement. For longer tasks, the browser reconnects with `Last-Event-ID` or uses the signed
direct-stream fallback.

## Persistence

| Lives in                | Tables / keys                                | Lifetime                                                         |
| ----------------------- | -------------------------------------------- | ---------------------------------------------------------------- |
| NeonDB                  | `agent_remote_dev_threads`                   | per-user, durable                                                |
| NeonDB                  | `agent_remote_dev_tasks`                     | per-thread, durable                                              |
| NeonDB                  | `agent_remote_dev_events`                    | append-only, durable                                             |
| NeonDB                  | `agent_remote_dev_artifacts`                 | per-task, durable                                                |
| Redis (Upstash)         | `remote-dev:task:<taskId>`                   | 24h hot cache                                                    |
| Redis (Upstash)         | `remote-dev:user-active:<userId>`            | 24h                                                              |
| Docker container memory | per-task event buffer + child process handle | until container restarts (then NeonDB takes over via SSE replay) |

Schema lives in
[`../src/server/databases/neondb/tables/agent-remote-dev-table.ts`](../src/server/databases/neondb/tables/agent-remote-dev-table.ts).

## Required environment variables

The full reference is in [`dev-server/readme.md`](./dev-server/readme.md). Quick sanity-check table
for core k8s routing + shared secrets:

| Var                                                                  | Where set           | Purpose                                                                                                                                                          |
| -------------------------------------------------------------------- | ------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `K8S_API_SERVER`                                                     | Vercel only         | Required. Enables k8s thread routing; dispatch is disabled without it.                                                                                           |
| `K8S_SA_TOKEN`                                                       | Vercel only         | Required. Service-account bearer token used for k8s API calls.                                                                                                   |
| `K8S_NAMESPACE`                                                      | Vercel only         | Optional namespace override (defaults from code).                                                                                                                |
| `K8S_INGRESS_HOST`                                                   | Vercel only         | Public host for path-based browser SSE (`/dd-thread/<short>`).                                                                                                   |
| `REMOTE_DEV_SERVER_PUBLIC_URL_TEMPLATE`                              | Vercel only         | Optional deterministic per-thread public direct-stream URL template (`https://{threadId}.…`).                                                                    |
| `REMOTE_DEV_THREAD_PROVISION_URL`                                    | Vercel only         | Optional idempotent provisioner endpoint. Given `{ threadId, taskId, userId }`, it starts/reuses the thread container and returns `{ baseUrl, publicBaseUrl? }`. |
| `REMOTE_DEV_THREAD_PROVISION_SECRET`                                 | Vercel only         | Optional `X-Provision-Auth` secret for that provisioner.                                                                                                         |
| `REMOTE_DEV_SERVER_SECRET` (Vercel) ↔ `SERVER_AUTH_SECRET` (Docker)  | both                | `X-Server-Auth` shared secret.                                                                                                                                   |
| `REMOTE_DEV_INGEST_SECRET` (Vercel) ↔ `EVENT_INGEST_SECRET` (Docker) | both                | `X-Agent-Auth` for events ingest.                                                                                                                                |
| `REMOTE_DEV_TOKEN_SECRET`                                            | both, identical     | HMAC-SHA256 secret for direct-stream tokens. ≥ 32 chars.                                                                                                         |
| `REMOTE_DEV_HEARTBEAT_SECRET` (Vercel) ↔ `HEARTBEAT_SECRET` (Docker) | both                | `X-Heartbeat-Auth` for periodic docker→Vercel check-ins.                                                                                                         |
| `REMOTE_DEV_REAPER_SECRET`                                           | Vercel + reaper pod | Shared `X-Reaper-Auth` secret for `/api/admin/remote-dev/reaper/sweep`.                                                                                          |
| `REMOTE_DEV_IDLE_TIMEOUT_MS`                                         | Vercel only         | Idle threshold before reaper scales thread deployments to `replicas=0` (default 600000).                                                                         |
| `SUPABASE_URL` / `SUPABASE_SERVICE_ROLE_KEY`                         | docker only         | So the docker can broadcast to per-user Realtime channels.                                                                                                       |
| `NEXT_PUBLIC_SUPABASE_URL` / `NEXT_PUBLIC_SUPABASE_ANON_KEY`         | Vercel only         | So the browser can subscribe directly to its own channel.                                                                                                        |

Plus, on the docker side:

- `ANTHROPIC_API_KEY`
- `GH_DEPLOY_KEY` + `GH_PAT` (so `git push` and `gh pr create` work)
- `EVENT_INGEST_URL` = `https://<your-vercel-app>/api/admin/remote-dev/events`
- `HEARTBEAT_URL` = `https://<your-vercel-app>/api/admin/remote-dev/heartbeat`
- `HEARTBEAT_INTERVAL_MS` (default 20000)
- `DEFAULT_STORAGE_PROVIDER` + the matching provider block (`S3_*` / `R2_*` / `GCS_*` / `DRIVE_*` /
  `LOCAL_STORAGE_*`)

On the scheduler side (Rust reaper pod or linux cron-service pod):

- `REAPER_SWEEP_URL` = `https://<your-vercel-app>/api/admin/remote-dev/reaper/sweep`
- `REAPER_SECRET` = same value as `REMOTE_DEV_REAPER_SECRET`

The live `dd-idle-reaper` pod also owns the 90-minute cluster doctor loop:

- `CLUSTER_DOCTOR_ENABLED=true`
- `CLUSTER_DOCTOR_INTERVAL_SECONDS=5400`
- `CLUSTER_DOCTOR_TASK_URL=http://dd-dev-server-api.default.svc.cluster.local:8080/tasks`
- `CLUSTER_DOCTOR_THREAD_ID=00000000-0000-4000-8000-000000000001`

The prompt is inline in [`idle-reaper-rs/src/main.rs`](./idle-reaper-rs/src/main.rs) for now. It
asks the remote-dev agent to query Prometheus, Loki, Grafana, NATS, and runtime health endpoints,
patch concrete issues under `remote/`, run targeted tests, and rely on `remote/dev-server` to
push/open the PR.

The same `dd-idle-reaper` deployment owns the daily worker image cron. At 4am America/New_York it
fast-forwards the EC2 checkout and rebuilds `docker.io/library/dd-dev-server:dev` with
`nerdctl -n k8s.io build`, so future thread pods pick up a fresh local image based on latest `dev`.

## Per-user channel security

The Supabase channel name is `remote-dev:user:<ddUserId>` — any other admin won't see another
admin's events because they subscribe with their own UUID. Three layers of defence:

1. **Page-level admin gate.** `/u/admin/remote-dev` and `/u/admin/remote-ui` require Clerk admin
   auth on the server before the page even renders. A non-admin can't read their own channel name
   off the server-rendered HTML.
2. **Channel-name-as-secret.** `ddUserId` is a UUID. An admin who already owns admin privileges
   learning _another_ admin's UUID would have to find it via SQL or another exploit; they won't get
   it from the UI.
3. **Tighten with Supabase Private Channels** when this graduates from single-tenant. The existing
   `broadcastManager` is wired through `supabase.channel(name)` so swapping in `private=true`
   channels + RLS-backed access tokens is one config change.

## Agent providers (pluggable)

Each task can be driven by Gemini, Claude, or OpenAI. The default is the OpenAI SDK runner;
override per dispatch (UI picker / API `provider` field) or globally via `AGENT_PROVIDER`
env on the docker. Failed runs retry through the configured fallback providers, with
`openai-sdk` primary and `claude-sdk` secondary by default.

| Provider           | Path                                                | Status               |
| ------------------ | --------------------------------------------------- | -------------------- |
| `gemini-sdk`       | `@google/genai` streaming SDK                       | model-only response runner |
| `claude-sdk`       | `@anthropic-ai/claude-agent-sdk` `query()` iterator | working              |
| `claude-cli`       | `claude -p ... --output-format stream-json`         | working CLI fallback |
| `openai-sdk`       | `@openai/agents` + scoped shell/apply-patch tools   | working default SDK path |
| `openai-codex-cli` | `codex exec "<prompt>" --json`                      | working CLI fallback |

Each runner gets a **strict env allowlist** — only the API key it needs (`GOOGLE_API_KEY` or
`GEMINI_API_KEY` for `gemini-sdk`, `ANTHROPIC_API_KEY` for `claude-*`, `OPENAI_API_KEY` for `openai-*`), model pins such as `GEMINI_MODEL` / `GEMINI_FALLBACK_MODEL`, `PATH`, `HOME`, `USER`, `LANG`, `NODE_ENV`. The agent
process never sees the GitHub PAT, deploy key, Supabase service role key, or `REMOTE_DEV_*` shared
secrets.

## What's working today vs stubbed

| Component                                                                                      | Status                                                                                    |
| ---------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| Server core (HTTP, SSE, child process orchestration)                                           | working                                                                                   |
| Per-thread git workspaces + `pnpm install`                                                     | working                                                                                   |
| Agent runner abstraction (Claude/OpenAI × CLI/SDK)                                             | working — see provider matrix above                                                       |
| Claude SDK/CLI + OpenAI SDK/CLI streaming                                                      | working                                                                                   |
| `git push` + PR create/reuse                                                                   | working                                                                                   |
| HMAC direct-stream tokens (Vercel signer + docker verifier)                                    | working                                                                                   |
| Vercel `/api/admin/remote-dev/*` routes                                                        | working                                                                                   |
| `/u/admin/remote-dev` chat UI (Supabase Realtime + RxJS SSE fallback + replay + adaptive poll) | working                                                                                   |
| `/u/admin/remote-ui` thread browser                                                            | working                                                                                   |
| Heartbeat: docker → Vercel + `/docker-health` route                                            | working                                                                                   |
| End-thread route + UI button (cancels active tasks + archives)                                 | working                                                                                   |
| Thread lifecycle controls: pause/sleep, archive, runtime delete                                | working in Next admin UI and Rust `/agents/tasks` UI                                      |
| Merge latest `origin/dev` into a thread branch                                                 | working via Next admin UI, Rust `/agents/tasks`, and worker `POST /thread/merge-upstream` |
| Editable task prompts + upvote/downvote response feedback                                      | working through the existing task `meta` JSONB column; no DB migration required           |
| Supabase Realtime fan-out (per-user channel, lambda-independent)                               | working                                                                                   |
| Storage adapter — `local`                                                                      | working (dev only)                                                                        |
| Storage adapter — `s3` / `r2`                                                                  | scaffolded; install `@aws-sdk/client-s3` and replace the TODO block                       |
| Storage adapter — `gcs`                                                                        | scaffolded; install `@google-cloud/storage` and wire upload                               |
| Storage adapter — `drive`                                                                      | scaffolded; install `googleapis` and wire upload                                          |

The structural plumbing — schema, zod shapes, event flow, UI, auth, thread runtime routing, and
SDK/CLI provider selection — is in place end-to-end. The only thing between us and a real cloud
upload is the storage SDK install + adapter implementation, all clearly marked `TODO(remote-dev)`
in [`dev-server/src/storage/`](./dev-server/src/storage/).

## Deployment cadence and container shape

The warm-image path is now the default for Kubernetes: `remote/dev-server` builds an image with
git, OpenSSH, GitHub CLI, provider CLIs, the compiled Node server, and a warm `dd-next-1`
repo-template already installed. The image runs as the built-in `node` user and stores mutable
thread state under `/home/node/workspace`.

The EC2/containerd tag used by the current cluster is `docker.io/library/dd-dev-server:dev`.
Per-thread Deployments reuse that baked image, mount a per-thread workspace, and let the entrypoint
seed `/home/node/workspace/repo` from `/home/node/repo-template` before fetching the latest
`origin/dev`.

The part to be careful with is "new container for every HTTP request". That is not a good fit for
SSE, heartbeat, cancel, snapshot, and event ingest calls because those calls are part of one
long-running task conversation. The current shape is:

- **One K8s worker Deployment per thread UUID**: scale between `0` and `1` to sleep/wake the
  thread.
- **A durable workspace per thread**: the mounted workspace keeps branch state and local artifacts
  across sleep/wake cycles.
- **NeonDB/RDS + NATS/WebSocket fan-out**: durable event log plus low-latency browser stream.

## Infra automation path (AMI + IaC)

- AMI build + cluster bootstrap live in [`./ami/`](./ami/) and [`./ec2/`](./ec2/).
- `remote/ami` pre-installs cluster/runtime tooling (including Terraform and AWS CDK) so a launched
  node is a repeatable long-term base image.
- `remote/ec2/bootstrap-amazon-linux-2023-k8s.sh` is the stock-EC2 fallback when AMI baking is
  unavailable.
- Target machine profile for the long-lived node: about 8 vCPU, 100-120 GiB RAM, and 800 GiB disk
  (see [`ec2/README.md`](./ec2/README.md)).

## Known accepted risks (not yet mitigated)

These are recognised tradeoffs we explicitly chose not to harden in v1. Each one is a real attack
surface — review before broadening access.

- **Agent shell/file tools can make broad branch edits.** SDK providers are scoped to the thread
  workspace and use explicit tool surfaces; CLI providers still expose broad shell access inside
  that workspace. The blast radius is bounded to the thread branch (`agent/k8s/openai-5.5/<threadId>/<slug>`)
  and PR review gate, not direct `dev` writes.
- **No per-user concurrent-stream cap.** A single admin opening many task tabs each starts a
  250ms-poll SSE against Postgres for up to ~12 min. Mitigate later with a Redis semaphore (cap N
  concurrent streams per user) once usage scales. Not exploitable cross-user.
- **Prompts in logs and PR bodies.** User prompts are appended to `tmp/convos/thread.log`, passed
  through provider streams, and included in PR bodies. Admins occasionally paste secrets into
  prompts ("debug this with key X"). Treat docker logs and PR bodies as "secret-handling-equivalent
  surfaces" until we add prompt redaction.
- **Channel-name-as-secret.** Per-user Supabase Broadcast channels use the dd-user UUID as the
  channel name. Another admin who learned a victim's UUID via SQL/log scraping COULD subscribe and
  observe events. Tighten by switching to Supabase Private Channels with RLS-backed subscriber
  tokens (the existing `broadcastManager` is already routed through `supabase.channel(...)`, so the
  swap is one config change).
- **Event ingestion auth = shared secret only.** `/api/admin/remote-dev/events` authenticates with
  a single `X-Agent-Auth` header. If the secret leaks (env var dumped, image extracted), an
  attacker can inject arbitrary `done` / `artifact` events for any taskId. URLs in those events are
  now refined to http(s)-only (no `javascript:`) but the event log can still be polluted. Future:
  HMAC-sign the body with per-task derivation so injected events for unknown tasks 4xx.
- **Image storage adapters S3/R2/GCS/Drive are stubs.** They throw on call until the corresponding
  SDK is wired in (`@aws-sdk/client-s3`, `@google-cloud/storage`, `googleapis`). Each TODO block in
  [`dev-server/src/storage/`](./dev-server/src/storage/) has the exact code to drop in.

## Adding more services here later

A new long-running service should be a sibling directory with its own `Dockerfile`, `package.json`,
and `readme.md` — for example, `remote/queue-worker/` or `remote/eval-runner/`. Try to reuse the
same patterns dev-server uses:

- All credentials in `process.env`, never baked into the image.
- Talk to Vercel via shared-secret HTTP, not direct DB access.
- Persist nothing locally that we'd cry about losing on restart; NeonDB is the system of record.
- One image, no monorepo wiring inside the container.
