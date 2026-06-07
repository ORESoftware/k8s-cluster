# `remote/argocd/dd-next-runtime`

GitOps manifests for the baseline runtime that should always be visible in Argo:

- `dd-remote-web-home` (Rust web layer for `/` + `/home`)
- `dd-remote-auth` (Rust PIN auth service that sets the gateway `dd_auth` cookie)
- `dd-remote-rest-api` (Rust RDS/Postgres REST API for agent task data)
- `dd-agent-worker-broker` (Rust worker-dispatch broker that chooses direct-if-awake or NATS queue)
- `dd-container-pool` (Rust Postgres-configured warm container pool over HTTP or NATS)
- `dd-build-server` (Rust CI/CD build server for repo image builds and controlled k8s deploys)
- `dd-gleam-lambda-runner` (Gleam child-process runner for user-defined lambda invocations)
- `dd-remote-queue-consumer` (Rust NATS queue consumer for UUID-bound workers and explicit pool dispatch)
- `dd-webrtc-signaling` (Rust WebRTC room signaling over WebSocket)
- `dd-webrtc-media` (Rust WebRTC ICE/TURN/SFU/media-relay configuration surface)
- `dd-web-scraper` (Node.js/Fastify scraping worker with browser and DOM strategies)
- `dd-browser-test-server` (Node.js/Fastify on-demand Playwright + Puppeteer + Selenium runner)
- `dd-selenium-server` (Java/Vert.x + selenium-java API driving an in-pod Selenium Grid over RemoteWebDriver)
- `dd-browser-job-runner` (Rust/axum orchestrator that runs one Playwright/Puppeteer job on a dd-container-pool warm worker, falling back to a direct nerdctl worker, and publishes results to NATS)
- `dd-live-mutex` (single-broker Live-Mutex TCP service for cluster-local locking)
- `dd-ai-ml-pipeline` (Python3 online telemetry feature pipeline in the `ai-ml` namespace)
- `dd-des-simulator` (Rust asynchronous discrete event simulation service with `des.v1` model validation)
- `dd-fabrication-server` (Rust fabrication planner and instruction validator for printers, mills, routers, and lathes)
- `dd-contract-service` (Rust Solana contract gateway for `solana.contract.v1` validation)
- `dd-escrow-rs` (Rust Solana escrow gateway for `solana.escrow.v1` validation and settlement)
- `dd-trading-server` (Rust trading decision service for `trading.decision.v1` risk-gated order intents)
- `dd-economics-server` (Rust economics dashboard and `economics.forecast.v1` theory/data projection service)
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
- `/api/agents/*` -> `dd-remote-rest-api:8082` (gateway auth required)
- `/api/agents/threads/<uuid>/prepare` -> `dd-remote-rest-api:8082` (internal auth required)
- `POST /api/agent-worker/threads/<uuid>/tasks` -> `dd-agent-worker-broker:8098`
- `/container-pools`, `/container-pools/<pool>`,
  `POST /container-pools/<pool>/dispatch` -> `dd-container-pool:8102`
  (internal auth required)
- `/builds`, `/builds/<jobId>`, `/builds/<jobId>/logs` -> `dd-build-server:8100`
  (internal auth required)
- `/lambdas/functions` -> `dd-remote-web-home:8080`
- `/api/lambdas/functions` -> `dd-remote-rest-api:8082`
- `/api/db/*` -> `dd-remote-rest-api:8082` (server auth required; generic RDS table access)
- `POST /lambdas/invoke/<function-id>` -> `dd-gleam-lambda-runner:8083` directly
- `/webrtc/`, `/webrtc/healthz`, `/webrtc/metrics`, `/webrtc/signal` ->
  `dd-webrtc-signaling:8095` (gateway auth required)
- `/webrtc-media/`, `/webrtc-media/config`, `/webrtc-media/ice`, `/webrtc-media/metrics` ->
  `dd-webrtc-media:8125` (gateway auth required; HTTP config only, no UDP media gatewaying)
- `/presence/`, `/presence/healthz`, `/presence/ws`, `/presence/conv/*`, `/presence/user/*` ->
  `presence-svc.presence:8081` (gateway auth required)
- `/fsws/`, `/fsws/healthz`, `/fsws/livez`, `/fsws/ws/*` -> `dd-fsharp-ws-server:8087`
  (gateway auth required)
- `/gcs/health`, `/gcs/ws-health`, `/gcs/api/*`, `/gcs/ws/*` -> `gcs` / `gcs-router`
  (gateway auth required)
- `/des/`, `/des/model/schema`, `/des/model/example`, `POST /des/validate`,
  `POST /des/simulate`, `/des/simulations/<jobId>` -> `dd-des-simulator:8099`
  (gateway auth required)
- `/mdp/`, `/mdp/healthz`, `/mdp/metrics`, `POST /mdp/optimize`,
  `POST /mdp/telemetry/learn` -> `dd-mdp-optimizer:8096` (gateway auth required)
- `/fabrication/`, `/fabrication/healthz`, `/fabrication/metrics`, `/fabrication/docs/api`,
  `/fabrication/capabilities`, `/fabrication/schema`, `/fabrication/examples`,
  `/fabrication/machines/catalog`, `/fabrication/printers/catalog`,
  `/fabrication/subtractive/catalog`, `/fabrication/subtractive/preflight/catalog`,
  `/fabrication/cleanliness/preflight/catalog`,
  `/fabrication/interfaces/preflight/catalog`, `/fabrication/cnc/catalog`,
  `/fabrication/hybrid/catalog`, `/fabrication/cells/catalog`,
  `POST /fabrication/machines/select`,
  `/fabrication/controllers/catalog`, `POST /fabrication/controllers/result`,
  `/fabrication/materials/catalog`, `POST /fabrication/materials/plan`,
  `POST /fabrication/materials/result`,
  `/fabrication/design/formats`, `/fabrication/slicers/catalog`,
  `POST /fabrication/slicers/result`,
  `/fabrication/mesh-repair/catalog`, `POST /fabrication/mesh-repair/result`,
  `/fabrication/design/import/catalog`,
  `/fabrication/design/preflight/catalog`,
  `POST /fabrication/design/import/review`,
  `POST /fabrication/design/import/result`,
  `POST /fabrication/design/convert/plan`, `POST /fabrication/design/convert/result`,
  `/fabrication/design/generation/catalog`, `POST /fabrication/design/generate`,
  `POST /fabrication/design/synthesis/result`,
  `/fabrication/handoff/catalog`, `POST /fabrication/handoff/result`,
  `/fabrication/subjects/catalog`, `/fabrication/workers/catalog`,
  `/fabrication/results/catalog`,
  `/fabrication/instructions/languages`, `/fabrication/instructions/import/catalog`,
  `/fabrication/instructions/import/preflight/catalog`,
  `/fabrication/instructions/validation/catalog`,
  `/fabrication/instructions/generation/catalog`,
  `/fabrication/instructions/generation/preflight/catalog`,
  `POST /fabrication/instructions/generate`,
  `POST /fabrication/instructions/generation/result`,
  `POST /fabrication/instructions/review/result`,
  `POST /fabrication/instructions/validation/result`,
  `/fabrication/machine-code/catalog`,
  `POST /fabrication/machine-code/generate`,
  `POST /fabrication/machine-code/result`, `/fabrication/toolpaths/catalog`,
  `POST /fabrication/toolpaths/plan`,
  `POST /fabrication/toolpaths/result`,
  `/fabrication/improvements/catalog`, `/fabrication/improvements/preflight/catalog`,
  `/fabrication/boundaries/catalog`, `/fabrication/boundaries/preflight/catalog`,
  `/fabrication/remediation/catalog`,
  `POST /fabrication/remediation/plan`, `POST /fabrication/remediation/result`,
  `/fabrication/decomposition/catalog`,
  `POST /fabrication/decomposition/plan`, `POST /fabrication/decomposition/result`,
  `/fabrication/assembly/catalog`, `/fabrication/assembly/preflight/catalog`,
  `POST /fabrication/interfaces/result`,
  `POST /fabrication/assembly/plan`,
  `POST /fabrication/assembly/result`,
  `/fabrication/release/catalog`, `/fabrication/release/preflight/catalog`,
  `POST /fabrication/release/preview`,
  `POST /fabrication/release/result`,
  `/fabrication/strategy/catalog`, `/fabrication/methods/catalog`,
  `POST /fabrication/strategy/recommend`, `POST /fabrication/strategy/result`,
  `/fabrication/schedule/catalog`, `POST /fabrication/schedule/result`,
  `POST /fabrication/execution/plan`, `POST /fabrication/execution/result`,
  `/fabrication/simulation/catalog`, `/fabrication/simulation/preflight/catalog`,
  `POST /fabrication/simulation/run`,
  `POST /fabrication/simulation/result`,
  `/fabrication/quality/catalog`, `/fabrication/quality/preflight/catalog`,
  `/fabrication/dispositions/catalog`,
  `POST /fabrication/dispositions/result`, `/fabrication/costing/catalog`,
  `POST /fabrication/costing/result`, `/fabrication/utilities/catalog`,
  `/fabrication/energy/catalog`, `POST /fabrication/energy/result`,
  `POST /fabrication/utilities/result`,
  `/fabrication/telemetry/catalog`, `/fabrication/availability/catalog`,
  `POST /fabrication/availability/result`, `/fabrication/maintenance/catalog`,
  `POST /fabrication/maintenance/result`,
  `POST /fabrication/telemetry/result`,
  `POST /fabrication/quality/plan`,
  `POST /fabrication/quality/result`, `POST /fabrication/manufacturability/result`,
  `/fabrication/calibration/catalog`, `POST /fabrication/calibration/plan`,
  `POST /fabrication/calibration/result`,
  `/fabrication/interventions/catalog`, `POST /fabrication/interventions/result`,
  `/fabrication/setup/catalog`,
  `/fabrication/tooling/catalog`, `POST /fabrication/tooling/result`,
  `/fabrication/consumables/catalog`,
  `POST /fabrication/consumables/result`,
  `/fabrication/workholding/catalog`,
  `/fabrication/workholding/preflight/catalog`,
  `POST /fabrication/workholding/result`,
  `/fabrication/nesting/catalog`, `POST /fabrication/nesting/result`,
  `/fabrication/support-strategies/catalog`, `POST /fabrication/support-strategies/result`,
  `/fabrication/process-recipes/catalog`, `POST /fabrication/process-recipes/result`,
  `/fabrication/kinematics/catalog`, `POST /fabrication/kinematics/result`,
  `/fabrication/tolerances/catalog`, `POST /fabrication/tolerances/result`,
  `/fabrication/process-capabilities/catalog`, `POST /fabrication/process-capabilities/result`,
  `/fabrication/manufacturability/catalog`,
  `/fabrication/failure-modes/catalog`, `POST /fabrication/failure-modes/result`,
  `/fabrication/safety/catalog`, `POST /fabrication/safety/result`,
  `/fabrication/environment/catalog`, `POST /fabrication/environment/result`,
  `/fabrication/provenance/catalog`, `/fabrication/as-built/catalog`,
  `POST /fabrication/as-built/result`, `POST /fabrication/provenance/result`,
  `POST /fabrication/setup/plan`,
  `POST /fabrication/setup/result`,
  `/fabrication/monitoring/catalog`, `POST /fabrication/monitoring/plan`,
  `POST /fabrication/monitoring/result`,
  `/fabrication/postprocess/catalog`, `POST /fabrication/postprocess/plan`,
  `POST /fabrication/postprocess/result`,
  `/fabrication/artifacts/catalog`, `/fabrication/jobs/catalog`,
  `/fabrication/learning/capabilities`, `/fabrication/learning/engines/catalog`,
  `/fabrication/learning/preflight/catalog`, `/fabrication/learning/rewards/catalog`,
  `/fabrication/learning/models/catalog`,
  `/fabrication/learning/optimizers/catalog`, `POST /fabrication/learning/models/result`,
  `POST /fabrication/learning/optimizers/result`,
  `/fabrication/jobs`, `/fabrication/jobs/<jobId>`,
  `/fabrication/jobs/<jobId>/release-bundle`,
  `/fabrication/jobs/<jobId>/artifacts/<artifactId>`, `/fabrication/learning/policy`,
  `/fabrication/learning/corpus`, `GET /fabrication/learning/outcomes`, `POST /fabrication/learning/observe`,
  `POST /fabrication/learning/outcomes`, `POST /fabrication/plan`,
  `/fabrication/workflow/catalog`, `POST /fabrication/workflow/plan`,
  `POST /fabrication/instructions/analyze`, `POST /fabrication/instructions/validate`,
  `POST /fabrication/instructions/improve`,
  `POST /fabrication/instructions/boundaries/review`,
  `POST /fabrication/remediation/plan`, `POST /fabrication/remediation/result` ->
  `dd-fabrication-server:8113` (gateway auth required)
- `/grafana/fabrication` -> `dd-remote-web-home:8080` redirect to the
  `dd-fabrication-planner` Grafana dashboard
- `/contracts/`, `/contracts/schema`, `/contracts/example`, `POST /contracts/validate`,
  `POST /contracts/simulate`, `POST /contracts/send` -> `dd-contract-service:8101`
  (internal auth required)
- `/escrow/`, `/escrow/types`, `/escrow/capabilities`, `/escrow/schema`, `/escrow/example`,
  `POST /escrow/validate`, `POST /escrow/audit`, `POST /escrow/simulate-settlement`,
  `POST /escrow/settle` -> `dd-escrow-rs:8115`
  (internal auth required)
- `/ml/`, `/ml/healthz`, `/ml/metrics`, `/ml/status`, `POST /ml/analyze`, `POST /ml/ingest` ->
  `dd-ai-ml-pipeline.ai-ml:8099` (internal auth required)
- `/trading/`, `/trading/schema`, `/trading/example`, `POST /trading/decide` ->
  `dd-trading-server:8103` (internal auth required)
- `/economics/`, `/economics/healthz`, `/economics/readyz`, `/economics/metrics`,
  `/economics/observability`, `/economics/dashboard.json`, `/economics/model/equations`,
  `/economics/sources`, `/economics/sources/public`, `POST /economics/forecast`,
  `POST /economics/ingest`, `POST /economics/sources/pull`, `/economics/sentiment/sources`,
  `POST /economics/sentiment/analyze`, `/economics/macro/indicators`,
  `/economics/vc/investment`, `POST /economics/recommendations`,
  `/economics/audit/hardening`, `/economics/pipelines/catalog`,
  `POST /economics/pipelines/plan`, `POST /economics/pipelines/submit` ->
  `dd-economics-server:8114` (internal auth required)
- `/scrape`, `/scrape/strategies`, `/scrape/healthz`, `/scrape/metrics` -> `dd-web-scraper:8097`
  (internal auth required)
- `/browser-test`, `/browser-test/healthz`, `/browser-test/metrics`, `/browser-test/status`,
  `/browser-test/tools` -> `dd-browser-test-server:8104` (internal auth required;
  `POST /run` accepts a bounded scenario DSL across Playwright, Puppeteer, and Selenium)
- `/selenium`, `/selenium/healthz`, `/selenium/metrics`, `/selenium/status`, `/selenium/tools` ->
  `dd-selenium-server:8105` (internal auth required; `POST /run` accepts the same bounded scenario
  DSL but Selenium-only, driving an in-pod Selenium Grid over RemoteWebDriver)
- `/browser-jobs`, `/browser-jobs/healthz`, `/browser-jobs/metrics`, `/browser-jobs/status`,
  `/browser-jobs/jobs`, `/browser-jobs/tools` -> `dd-browser-job-runner:8106` (internal auth required;
  `POST /run` spawns ONE ephemeral Playwright/Puppeteer worker container per job and returns a `jobId`
  plus NATS result subject immediately — results are published to NATS, not the HTTP response)
- `/tasks`, `/stream`, `/status`, `/agents`, `/healthz` -> bootstrap `dd-dev-server-api:8080`
- `/dd-thread/<short>/...` -> target per-thread Kubernetes Ingress shape; the selected Node.js
  worker is pinned to one thread and does not route UUIDs itself. `/dd-thread/<short>/ws` is the
  direct worker WebSocket for replay/live task events; both routes require gateway auth before the
  gateway injects the worker `X-Server-Auth` header.

Availability guardrail for gateway-backed HTTP services: request-serving deployments that are safe
to run in parallel should keep `replicas: 2`, `minReadySeconds: 5`, `progressDeadlineSeconds: 1800`,
rolling updates with `maxUnavailable: 0` / `maxSurge: 1`, readiness probes, and a
`PodDisruptionBudget` with `minAvailable: 1`. This covers the public/auth/API surface where normal
rollouts previously caused intermittent gateway `502`s: `dd-remote-web-home`, `dd-remote-auth`,
`dd-remote-rest-api`, `dd-agent-worker-broker`, `dd-des-rs`, `dd-contract-service`,
`dd-escrow-rs`, `dd-mdp-optimizer`, `dd-fabrication-server`, `dd-trading-server`, `dd-economics-server`, `dd-web-scraper`, `dd-browser-test-server`,
`dd-selenium-server`, and `dd-rust-vapi-phone`. `dd-des-rs` also has a small HPA with
`minReplicas: 2` and a long scale-down stabilization window. That service cold-builds the Rust
server and DES engine inside the pod, so scale-to-zero drift creates a multi-minute no-endpoint
window and shows up at the gateway as `/des-rs/` `502`s.

`dd-fabrication-server` also carries explicit runtime hardening because fabrication planning can
fan out to NATS, runtime-config, MDP optimization, and external design/instruction references. Its
Service is an explicit cluster-local TCP `ClusterIP` with `appProtocol: http`, not-ready endpoints
unpublished, and three-hour `ClientIP` affinity so gateway follow-up reads for in-process job and
artifact records tend to land on the pod that accepted the original planning request. The Deployment uses
explicit HTTP startup/liveness probes on `/healthz`, HTTP `/readyz` readiness, revision history, host and zone topology spread, soft pod anti-affinity,
explicit non-host network/PID/IPC namespaces with pod process-namespace sharing disabled, pod/container non-root UID/GID `1000` defaults, dropped Linux capabilities, `RuntimeDefault` seccomp, a short `preStop` drain, and a named no-RBAC ServiceAccount with token automount disabled. The Deployment, pod template, Service, ServiceAccount, HPA, PDB, and NetworkPolicy also carry Kubernetes app metadata for name, component, and `dd-next-runtime` ownership so operator queries can target the fabrication planner without changing immutable selectors. It runs
with a read-only root filesystem and read-only source mounts limited to the fabrication crate plus
the local NATS subject, runtime-config client, and shared-interface dependency crates Cargo needs;
an init container checks that source layout before the release build starts and verifies the
generated API docs still include the public plan, instruction analysis, job/artifact retrieval, and
learning routes. It also checks the fabrication crate's local Cargo dependency paths for
`des_engine`, generated NATS subject defs, and the runtime-config client before the locked build
can start, then checks the mounted generated Rust NATS subject constants for fabrication
requests/results, CAD design-conversion request/queue/result handoffs, runtime events, and MDP
optimization fan-out, so missing hostPath dependencies, stale docs, or stale subject definitions fail with a clear startup log and tiny CPU/memory/ephemeral-storage bounds. Cargo cache/target output stays on pod-local `emptyDir` storage with explicit `4Gi`/`8Gi` ephemeral-storage request/limit settings plus
per-volume `sizeLimit` caps; a dedicated release-build init container performs the single-job
locked Cargo build into that shared target directory, then the serving container remounts that cache
read-only and starts the compiled server binary directly. Incremental compilation stays disabled for predictable cold-start memory and disk use. The pod reserves `250m` CPU and `512Mi` memory for
cold builds and planning bursts while the `2` CPU / `2Gi` memory limits still cap runaway work. Pod
DNS sets `ndots: 2` so NATS/runtime-config service lookups prefer the intended cluster FQDN before
trying extra search-domain expansions. Runtime-config pushes are explicitly fail-closed:
`RUNTIME_CONFIG_ALLOW_UNAUTHENTICATED=false`, so `/internal/update-runtime-config` requires the
shared `SERVER_AUTH_SECRET` via `X-Server-Auth` when runtime-config delivers a snapshot.
Cargo registry fetches use the sparse protocol plus bounded retry/timeout settings, the Deployment
progress deadline gives a cold locked release build room to finish before a rollout is marked stalled, and the startup probe
now gives the compiled server up to about five minutes to become healthy after the binary starts.
Pods require two consecutive `/readyz` successes before the Service routes fabrication traffic.
Source validation, release-build, and serving startup failures fall back to recent container logs in
the termination message, and pod annotations make the serving container the default for `kubectl`
logs/exec while init-container logs remain addressable by name. During pod termination, the
60-second grace window gives the serving entrypoint time to forward SIGTERM/SIGINT into the compiled
Rust server's graceful-shutdown path before the pod exits. The HPA keeps 2-8 replicas based on CPU/memory pressure, and the
immediate scale-up policy can double ready capacity during planning bursts while scale-down remains
one-pod-at-a-time after a five-minute stabilization window. The Kubernetes resource exporter also
publishes HPA current/desired/min/max replica state and an at-max signal so Prometheus can alert
when fabrication planning demand holds the autoscaler at its ceiling. The dedicated NetworkPolicy permits
only gateway, runtime-config, and observability ingress plus DNS, NATS, runtime-config, in-cluster
MDP optimizer HTTP, and public IPv4/IPv6 HTTP/S egress that excludes private and reserved ranges. The gateway keeps
`/fabrication/` uploads bounded at `512k` to match the Rust HTTP/NATS payload caps, disables
request buffering so submitted instruction payloads stream to the Rust service, and uses explicit
upload/connect plus 900-second upstream read/send timeouts for longer planning and analysis responses. It forwards the NGINX `X-Request-ID`, `X-Forwarded-For`, `X-Forwarded-Host`, `X-Forwarded-Port`, `X-Forwarded-Prefix: /fabrication`, `X-Forwarded-Proto`, `X-Original-URI`, and `X-Real-IP` values to the Rust service so edge failures and downstream job evidence can be correlated even before a payload-level `requestId` is parsed and after the public route prefix is stripped. It also disables upstream retries for `/fabrication/` so non-idempotent planning or instruction-submission requests are not replayed after an upstream failure. Public `/fabrication/internal` and `/fabrication/internal/*` paths return 404 at the gateway so service-local runtime-control routes stay off the Internet-facing surface. The canonical `/fabrication` redirect is read-only (`GET`/`HEAD`); the public `/fabrication/` gateway surface allows only `GET`, `HEAD`, and `POST`, returning 405 for other methods before they reach the Rust service. Fabrication `POST` traffic also has a gateway burst guard of `12r/m` per remote address with a burst of `6`, returning 429 before expensive planning, instruction analysis, NATS fan-out, or MDP publishing can pile up.
The gateway emits redacted JSON access logs with schema `dd.gateway.access.v1`, request IDs, statuses, upstream status/timing, and path-only `uri` values so fabrication guardrail decisions can be correlated in Loki without writing `Auth` headers, cookies, or query strings. Gateway-generated `/fabrication` redirects, internal-route 404s, auth failures, method denials, payload rejections, and rate-limit responses also return `X-Request-ID` so operators can match a client-visible edge failure to the gateway access log before the request reaches Rust. Because those fabrication-specific locations define their own headers, they explicitly preserve the gateway security header set (`Strict-Transport-Security`, `Content-Security-Policy`, `X-Frame-Options`, `X-Content-Type-Options`, and `Referrer-Policy`) alongside `X-Request-ID` instead of relying on NGINX inheritance. Private internal-route probes return JSON `not_found` 404 responses, unsupported methods return JSON `method_not_allowed` 405 responses with exact `Allow` recovery hints (`GET, HEAD` for the canonical `/fabrication` redirect and `GET, HEAD, POST` for `/fabrication/`), oversized fabrication submissions return a JSON `payload_too_large` 413, and burst-guarded writes return a JSON `rate_limited` 429 with `Retry-After: 60`, so clients can distinguish gateway request-envelope failures from Rust planner validation failures.
Prometheus and OTel keep static scrape jobs for `/metrics`; the Service also carries Prometheus
scrape annotations with an explicit HTTP scheme so future discovery-based scrapers can find the
same endpoint. OTel also discovers ready `dd-fabrication-server` pods directly as
`dd-fabrication-server-pods`, preserving replica-local job/artifact ledgers, learning memory,
failure-boundary counters, and NATS/MDP fan-out counters that Service-level scrapes can hide.
Prometheus alerts if those direct pod scrape targets disappear or go down while the service-level
scrape still looks healthy. Operators can open `/grafana/fabrication` from the web-home service
directory to land on the dedicated `dd-fabrication-planner` dashboard for request intake,
validation findings, machine-failure boundaries, required operator actions, fixture/setup
blockers, split/combine reviews, capabilities/schema/example discovery, CAD/design format
discovery, format-import catalog and preflight discovery, design-import review, design-import result review,
design-generation catalog discovery, strategy and calibration catalog discovery, validation-result and
worker result-review route traffic, instruction-improvement review, instruction-boundary review,
NATS/MDP fan-out, runtime-config delivery, HPA pressure, and logs.
The observability stack also scrapes `dd-runtime-config` metrics so missed subscriber registration,
configuration-entry, or push-delivery changes are visible alongside the fabrication planner, NATS,
and MDP optimizer dependency metrics, and alerts when runtime-config is down, has no stage
subscribers, records stage push errors, or fails to push stage snapshots specifically to the
`dd-fabrication-server` subscriber. Prometheus also alerts when the NATS or MDP optimizer dependency
scrape targets are down, covering the queue/result/event path and fabrication policy optimization
fan-out path separately from the Rust service's own health.
Prometheus alerts when the fabrication scrape target is down or
`dd_fabrication_server_errors_total` starts increasing. It also alerts when machine-failure boundary
findings, required operator actions, fixture/setup blockers, or split/combine reviews are
increasing, successful requests or queued NATS intake are not producing fabrication result or
MDP-learning fan-out, the bounded in-process job/artifact/learning ledgers approach eviction or
artifact evidence high-watermarks, or the Deployment has
unavailable replicas during a cold build, scheduling, or readiness problem. It also alerts on
serving-container restarts because retained job/artifact evidence, learning memory, NATS
subscriptions, and active planning work are in-process. Init-container waiting and restart alerts
separate source-layout validation or release-build startup failures from running-service failures.
`RUST_LOG=info` plus plain-text Cargo output keep stdout/stderr logs at a predictable runtime level. The init
containers and serving entrypoint emit explicit source-check, release-build, server-start, and
shutdown-forwarding markers so pod logs show whether a startup delay is source validation, Cargo
compilation, or the running Rust service. Those markers include downward-API pod name, pod UID,
namespace, and node identity so cold-build logs can be tied back to the exact runtime placement and
recreated pods are not confused with earlier rollout attempts.

Single-owner workloads stay intentionally one-replica/`Recreate`: the host-port gateway, Redis,
mutex brokers, the bootstrap workspace worker, containerd/build managers, the runtime-config push
controller, benchmark WebSocket pods, and in-memory signaling/job-state services. The
`dd-remote-queue-consumer` remains one replica at rest because KEDA owns burst scaling, but it uses
rolling updates with `maxUnavailable: 0` so a rollout brings up the replacement consumer before
terminating the old one. The gateway also retries transient upstream `502`/`503`/`504` failures
before surfacing them to the browser. Do not scale `dd-remote-gateway` above one pod on the current
single-node `hostPort` deployment; gateway HA needs either multiple nodes with a DaemonSet/load
balancer shape or an external load balancer in front of multiple gateway instances. Canary or
blue/green rollout policy should be added through Argo Rollouts or an equivalent controller; this
overlay currently uses native Kubernetes Deployment rolling updates only.

The Node.js worker image is built as `docker.io/library/dd-dev-server:dev` on the EC2 containerd
node. It contains git, OpenSSH, GitHub CLI, provider CLIs, the compiled
`remote/deployments/dev-server` server, and Node transport dependencies for NATS and outbound
WebSocket fanout. Repo-scoped pool workers receive `DD_REPO_URL`, `BASE_BRANCH`, provider config,
NATS config, WebSocket fanout config, and Git credentials at runtime; the image may carry a cache
seed, but a live worker must not depend on a baked repo checkout as its source of truth. The
container runs as the built-in `node` user; mounted workspaces live under `/home/node/workspace`.

Protected ops paths accept either the legacy `Auth` request header or the browser `dd_auth` cookie.
Browser document requests redirect to `/auth?return=<original path>`; API/curl callers still
receive the redacted JSON `{"error":"unauthorized","errMessage":"missing required dd header"}`
response. The operator passphrase, optional TOTP seed, and cookie value must be provided by the
`dd-remote-auth-secrets` Kubernetes secret, not committed to Git. When
`DD_AUTH_TOTP_SECRET_BASE32` is present, `/auth` requires both the passphrase and a current
six-digit one-time code before setting the short-lived browser cookie.

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
- model-provider keys like `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`, `OPENAI_API_KEY`,
  `OPENCODE_API_KEY`, `DEEPSEEK_API_KEY`, `DASHSCOPE_API_KEY`, and `XAI_API_KEY`
- GitHub credentials used by the remote dev worker entrypoint and PR creation path:
  `GH_DEPLOY_KEY`, optional `GH_DEPLOY_KEY_PUBLIC`, and optional `GH_PAT`
- `AKKA_LICENSE_KEY` for the optional Akka WebSocket comparison workload

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
  "AKKA_LICENSE_KEY": "optional-official-akka-license-key",
  "ANTHROPIC_API_KEY": "replace-me",
  "OPENCODE_API_KEY": "replace-me",
  "DEEPSEEK_API_KEYS_JSON": "[\"replace-me\"]",
  "DASHSCOPE_API_KEYS_JSON": "[\"replace-me\"]",
  "XAI_API_KEYS_JSON": "[\"replace-me\"]"
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
`/api/agents/tasks` and `/api/lambdas/functions` through the authenticated gateway directly. Lambda invocation
traffic uses `/lambdas/invoke/<function-id>` and goes from the gateway to the Gleam runner without
passing through the REST API. The runner reads `LAMBDA_DATABASE_URL` from
`dd-gleam-lambda-runner-secrets` and uses Postgres to resolve that immutable UUID to a lambda
definition.

Model-provider keys, GitHub credentials, gateway/shared-auth values, and REST database URLs should
be updated in AWS Secrets Manager, not in Git. The auth service consumes `DD_AUTH_PIN`,
`DD_AUTH_COOKIE_VALUE`, and optional `DD_AUTH_TOTP_SECRET_BASE32` from `dd-remote-auth-secrets`;
rotate those values through AWS Secrets Manager or the matching External Secrets path before
applying this deployment. After rotation, restart the deployments that consume the changed secret so
env vars reload.

## Reaper and cluster doctor

The reaper deployment runs `remote/deployments/idle-reaper-rs`.

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

Every 90 minutes it dispatches the inline prompt from `remote/deployments/idle-reaper-rs/src/main.rs` to
`dd-dev-server-api`. The agent inspects Prometheus, Loki, Grafana, NATS, and runtime service
health, then makes a narrow repo fix when there is an actionable issue. `remote/deployments/dev-server` pushes
the branch and opens/reuses the PR.

The same deployment also runs an adaptive NATS watchdog. It listens to copies of
`dd.remote.thread.*.tasks` and `dd.remote.events`; legacy shadow task messages idempotently call
`/api/agents/threads/<threadId>/prepare`, real queued `task.dispatch` messages are ignored by the
watchdog so they stay owned by `dd-remote-queue-consumer`, and event messages are reposted to the
Gleam websocket fanout endpoint. When a window has activity it checks again after 5 seconds; quiet
windows back off to 15 seconds.

The reaper also runs the runtime floor reconciler every 20 seconds. That loop creates or verifies
the JetStream `DD_REMOTE_TASKS` stream and durable consumer `dd-remote-thread-preparer`, keeps the
`dd-remote-queue-consumer` Deployment scaled to at least one replica, and calls `dd-container-pool`
to warm any configured pool whose idle workers are below `min_warm`.

The reaper is also the cron supervisor for the local worker image. Every day at 4am America/New_York
it fetches/fast-forwards the EC2 checkout, runs `nerdctl -n k8s.io build` for
`remote/deployments/dev-server`, and overwrites the local image tag `docker.io/library/dd-dev-server:dev`. New
thread workers use that tag via `imagePullPolicy: IfNotPresent`, so the next created pod picks up
the newest local image on the EC2 Kubernetes node. This is intentionally a Rust scheduler inside
the reaper deployment, not Linux `cron`/`at`; Kubernetes keeps the supervisor process alive, and
the deployment mounts the EC2 containerd socket plus `nerdctl` for the actual build.

`dd-headlamp-cron-sentinel` is a tiny native Kubernetes `CronJob` kept in this kustomization only
to make Headlamp's Jobs and Cron Jobs workload cards non-zero. It runs a no-op BusyBox pod and uses
`concurrencyPolicy: Forbid` so there is normally one active child `Job`; the real maintenance loops
above still live in `dd-idle-reaper`.

## NATS queued execution path

The runtime now uses a NATS queued execution path by default:

1. REST dispatch without `dispatchMode` resolves to `queued`; explicit `dispatchMode: "direct"`
   posts only to the selected thread worker and does not publish a task to NATS.
2. Queued dispatch publishes the task payload through JetStream to
   `dd.remote.thread.<threadId>.tasks`, emits `dd.remote.orchestrator.wakeup`, and returns `202`.
3. `dd-remote-queue-consumer` reads the durable pull consumer `dd-remote-thread-preparer` on the
   `DD_REMOTE_TASKS` stream.
4. KEDA watches that JetStream consumer lag and scales `dd-remote-queue-consumer` from 1 to 8 pods
   when pending messages build up, then returns to 1 after the stream drains.
5. The consumer dispatches plain queued `task.dispatch` messages to the UUID-bound deterministic
   worker. Explicit pool modes (`queued-pool`, `nats-pool`, `container-pool`, or `pool`) go to the
   repo-scoped container pool with `affinityKey=<threadId>` and can fall back to the deterministic
   worker only if the matching repo pool is unavailable or rejects the task. Legacy shadow messages
   are prepare-only.
6. The queue consumer stores taskId receipts under `/tmp/dd-remote-queue-consumer/tasks`; the
   Node.js worker also stores taskId receipts under its log directory. Repeated messages are
   accepted idempotently and do not start duplicate agent runs.

Task status visibility has a second lane that is deliberately separate from execution ownership.
The REST API persists queue status events and best-effort posts the same `task-event` JSON directly
to both `dd-gleamlang-server` `/broadcast` and `dd-webrtc-signaling` `/runtime/broadcast`. NATS event
fanout still works as the normal telemetry bus, but web-home can receive the initial queued/NATS
failure statuses over Gleam or Rust websocket fanout even when `dd.remote.events` is degraded.

This proves the queue handoff and warmup behavior without allowing arbitrary generic workers to
steal coding-agent execution. Plain queued `task.dispatch` messages stay on UUID-bound thread
workers; explicit pool messages are routed to repo-scoped Node chat/Claude warm pools with
`threadId` affinity.

`dd-agent-worker-broker` is the additive long-run replacement for REST-owned worker dispatch. Its
first route, `POST /api/agent-worker/threads/<threadId>/tasks`, direct-posts to the deterministic
Node.js worker only when that worker is already healthy. If the worker is not awake, it publishes
the task to JetStream, emits the wakeup subject, and returns `202 queued`; the queue consumer then
owns pool selection. It deliberately chooses direct or queued, never both for the same accepted task.

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
images under `remote/deployments/container-pool-rs/runtime-images` for `nodejs`, `rust`, `golang`, `python3`,
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

The runtime also includes `dd-mdp-optimizer`, a Rust MDP/POMDP/RL optimization service. The gateway
requires operator auth for `/mdp/healthz`, `/mdp/metrics`, `POST /mdp/optimize`, and
`POST /mdp/telemetry/learn`. It
queue-subscribes to `dd.remote.mdp.optimize` for explicit optimization jobs and
`dd.remote.telemetry.mdp` for app/infra telemetry snapshots, then publishes results to
`dd.remote.mdp.results` plus compact runtime events on `dd.remote.events`.

`dd-fabrication-server` is one producer of those explicit optimization jobs:
`POST /fabrication/plan` and `POST /fabrication/workflow/plan` build fabrication
learning contracts, and the deployment can publish an optimizer-shaped MDP request to
`dd.remote.mdp.optimize` when
`FABRICATION_MDP_AUTOPUBLISH=true`.

## Solana contract service

`dd-contract-service` runs `remote/deployments/contract-service-rs` as a Rust Solana contract gateway. It serves
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
emits compact lifecycle events on `dd.remote.events`. Invalid or oversized NATS validation messages
and result-publish failures also emit `dd.log.v1` stderr records and compact critical events on
`dd.remote.events.critical` when NATS is reachable. `/contracts/metrics` includes fixed-label
Prometheus counters for validation, Solana RPC method outcomes, NATS publish outcomes, send-auth
failures, and policy rejections; the service is scraped by both `dd-otel-collector` and
`dd-prometheus`, with Loki/Grafana coverage through the shared observability stack.

The pod runs as non-root with a read-only root filesystem, no mounted service-account token, and a
NetworkPolicy that keeps ingress to the gateway, runtime-config, and observability, while limiting
egress to DNS, NATS, runtime-config, and public HTTPS Solana RPC.

## Solana escrow service

`dd-escrow-rs` runs `remote/deployments/dd-escrow-rs` as a Rust Solana escrow intent gateway. It
serves `/escrow/healthz`, `/escrow/metrics`, `/escrow/status`, `/escrow/types`,
`/escrow/capabilities`, `/escrow/schema`, `/escrow/example`, `POST /escrow/validate`,
`POST /escrow/audit`, `POST /escrow/simulate-settlement`, and `POST /escrow/settle`. The service
validates `solana.escrow.v1` intents for ten escrow shapes: marketplace order, milestone, freelance
contract, digital delivery, OTC trade, rental deposit, bounty, subscription release, group buy, and
dispute resolution.

The service does not hold private keys or sign transactions. Settlement callers submit a signed
Solana transaction; `POST /escrow/simulate-settlement` calls `simulateTransaction`, while
`POST /escrow/settle` calls `sendTransaction` only when `SOLANA_SETTLEMENT_ENABLED=true` plus
`ESCROW_SETTLEMENT_AUTH_SECRET` are configured and the request includes `x-escrow-settlement-auth`.
Live settlement also requires an attached validated `intent` by default
(`ESCROW_SETTLEMENT_REQUIRE_INTENT=true`), mainnet settlement has a second
`SOLANA_MAINNET_SETTLEMENT_ENABLED=true` gate, and `ESCROW_ALLOWED_PROGRAM_IDS` can restrict
`settlementPlan.programId` to an operator-reviewed allowlist. Request `cluster` must match
`SOLANA_CLUSTER`, private RPC URLs require `SOLANA_ALLOW_PRIVATE_RPC=true`, and `skipPreflight`
requires `SOLANA_ALLOW_SKIP_PREFLIGHT=true`.

The deployment queue-subscribes to `dd.remote.escrow.solana.validate` with queue group
`dd-escrow-rs`, publishes validation results to `dd.remote.escrow.solana.results`, and emits compact
lifecycle events on `dd.remote.events`. Invalid or oversized NATS messages and settlement send
failures emit `dd.log.v1` stderr records and compact critical events on `dd.remote.events.critical`
when NATS is reachable. `/escrow/metrics` includes fixed-label Prometheus counters for intent
validation, settlement simulation/sending, Solana RPC outcomes, NATS publish outcomes,
settlement-auth failures, and policy rejections; the service is scraped by both `dd-otel-collector`
and `dd-prometheus`.

The pod runs as non-root with a read-only root filesystem, no mounted service-account token, and a
NetworkPolicy that keeps ingress to the gateway, runtime-config, and observability, while limiting
egress to DNS, NATS, runtime-config, and public HTTPS Solana RPC.

## AI/ML feature pipeline

`dd-ai-ml-pipeline` runs `remote/deployments/ai-ml-pipeline` as a long-lived Python3 service in the `ai-ml`
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

`dd-web-scraper` runs `remote/deployments/web-scraper-service` as a long-lived Node.js/Fastify service from the
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

## Selenium server

`dd-selenium-server` is a dedicated, long-lived, Selenium-only runtime. It is a single pod with two
containers: the official `selenium/standalone-chromium` image (the actual Selenium server / Grid on
`:4444`, owning Chromium + chromedriver) and a Java/Vert.x API container that self-builds
`remote/deployments/selenium-server` with Maven and drives the Grid through `selenium-java`
`RemoteWebDriver` at `localhost:4444`. The Grid port is never published on the Service, so the only
reachable entrypoint is the authenticated API on `:8105`.

The API exposes `GET /healthz`, `GET /readyz`, `GET /metrics`, `GET /status`, `GET /tools`, and
`POST /run`; the gateway mirrors those under `/selenium/...`. `POST /run` accepts the same bounded
scenario DSL as `dd-browser-test-server` (`goto`, `click`, `fill`, `select`, `press`,
`waitForSelector`, `waitForUrl`, `waitForTimeout`, `extractText`, `extractAttribute`, `screenshot`,
`evaluate`) and returns structured step logs, extracted values, screenshots, and console entries.
It fails closed when `SERVER_AUTH_SECRET` is missing, caps work at `SELENIUM_MAX_CONCURRENT` browser
sessions (returning 429 over the cap), bounds scenarios with `SELENIUM_MAX_STEPS` /
`SELENIUM_MAX_TIMEOUT_MS`, and keeps arbitrary in-page script execution opt-in behind
`SELENIUM_ALLOW_EVALUATE=false`.

## dd-browser-job-runner (per-job Playwright/Puppeteer, pool-first)

Where `dd-selenium-server` and `dd-browser-test-server` are long-lived in-process runners,
`dd-browser-job-runner` runs **one fresh browser per job** and always delivers the result over NATS.
Each `POST /run` returns a `jobId` immediately (HTTP 202); the work then runs on one of two paths. It
is a privileged, host-network Rust (axum) pod — the same posture as `dd-container-pool` and
`dd-gleam-lambda-runner`. Source: `remote/deployments/browser-job-runner-rs` (the orchestrator) and
`remote/deployments/browser-job-runner-rs/worker` (the dual-mode `dd-browser-job-worker` Node/TS image).

Request flow:

1. Operators `POST /run` (gateway `/browser-jobs/run`, internal auth required) with `{ engine,
   url?, steps[], viewport?, ... }`, where `engine` is `playwright` or `puppeteer` and `steps` is the
   same bounded DSL as `dd-browser-test-server` (`goto`, `click`, `fill`, `select`, `press`,
   `waitForSelector`, `waitForUrl`, `waitForTimeout`, `extractText`, `extractAttribute`, `screenshot`,
   `evaluate`).
2. It validates the job and responds **immediately** (HTTP 202) with `{ jobId, engine, resultSubject,
   eventsSubject, resultFanoutSubject, poolSubject, deadlineMs }`. **Results are not returned over
   HTTP.**
3. **Primary path — `dd-container-pool`:** the orchestrator NATS request/replies the pool subject
   `dd.remote.container_pool.browser-jobs.requests`. The `browser-jobs` pool (defined in
   `remote/databases/pg/seeds/container-pool-app-config.sql`, `minWarm 1 / maxWarm 3`,
   `requestTimeoutMs 540000`) keeps warm `dd-browser-job-worker` containers; it leases one,
   HTTP-dispatches the scenario to its `/run`, and replies with the worker's `RunResult`. The
   orchestrator republishes that to the per-job `dd.remote.browser_jobs.<jobId>.result` subject and the
   `dd.remote.browser_jobs.results` fanout. The warm worker **self-exits after one job**, so the pool
   retires it and reconciles a fresh replacement (one clean browser per job).
4. **Fallback path — direct nerdctl:** when the pool has no responders, errors, returns 409 (raced a
   just-used worker), or is saturated, the orchestrator spawns its own
   `nerdctl -n dd-browser-jobs run -d --rm --network host …` worker labelled `dd.browser-job.managed=true`
   with a `dd.browser-job.deadline-ms` no more than `BROWSER_JOB_MAX_LIFETIME_SECONDS` (**hard cap 540s
   / 9 min**) in the future, subject to `BROWSER_JOB_MAX_CONCURRENT`. That one-shot worker publishes its
   own result to NATS and exits. Host networking lets both pool and fallback workers reach the NATS
   ClusterIP exactly like the container-pool / lambda workers do.

Lifetime: pool containers are owned by `dd-container-pool` (retire-after-use + reconcile + idle TTL).
For the fallback path, three independent layers apply in the `dd-browser-jobs` namespace: the worker's
own watchdog hard-exits at `BROWSER_JOB_MAX_MS`; the orchestrator's tracker force-removes any container
past its deadline and prunes finished ones (keeping `GET /browser-jobs/jobs` accurate); and
`dd-idle-reaper` runs a `BROWSER_JOB_REAP_*` backstop loop that force-removes any
`dd.browser-job.managed=true` container that outlives its deadline label plus a grace, covering the
case where the orchestrator pod itself died.

The orchestrator fails closed when `SERVER_AUTH_SECRET` is missing (unless
`BROWSER_JOB_ALLOW_UNAUTHENTICATED=true`) and keeps arbitrary in-page script execution opt-in behind
`BROWSER_JOB_ALLOW_EVALUATE=false`. Set `BROWSER_JOB_POOL_ENABLED=false` to force the nerdctl path. The
worker image is pulled with `--pull=never`, so it must exist on the node — the pool builds the
`browser-jobs` `baseImages` entry into the `dd-pool` namespace, and the fallback uses `dd-browser-jobs`:

```
nerdctl -n dd-browser-jobs build -t docker.io/library/dd-browser-job-worker:dev \
  remote/deployments/browser-job-runner-rs/worker
nerdctl -n dd-pool build -t docker.io/library/dd-browser-job-worker:dev \
  remote/deployments/browser-job-runner-rs/worker
```
