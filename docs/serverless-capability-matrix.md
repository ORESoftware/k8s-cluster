# Serverless capability benchmark

Baseline: 2026-07-25. This is an implementation map, not a marketing comparison.
The reference set deliberately mixes managed functions, edge isolates, and a
Kubernetes-native system:

- [AWS Lambda](https://docs.aws.amazon.com/lambda/latest/dg/lambda-functions-chapter.html)
- [Google Cloud Run functions](https://docs.cloud.google.com/run/docs/functions/overview)
- [Cloudflare Workers](https://developers.cloudflare.com/workers/)
- [Azure Functions](https://learn.microsoft.com/azure/azure-functions/functions-overview)
- [Vercel Functions](https://vercel.com/docs/functions)
- [Deno Deploy](https://docs.deno.com/deploy/)
- [Knative](https://knative.dev/docs/)

Legend: **yes** is implemented in Scintilla now, **partial** has a usable subset,
and **gap** is not yet implemented.

| Capability synthesized from the reference platforms | Scintilla | Current implementation or next acceptance criterion |
| --- | --- | --- |
| HTTP function endpoint | **yes** | Authenticated direct invoke route plus control-plane facade |
| Multiple managed language runtimes | **yes** | Node.js, Python, Ruby, Bash, Go, Dart, Erlang, Elixir, Java, Gleam, Rust, browser |
| Custom runtime / OCI image | **yes** | Hardened container protocol with custom image and entry command |
| Warm instances / provisioned capacity | **yes** | Host and container prewarm lists; reusable workers |
| Process-level fault isolation | **yes** | Dynamic workers are temporary children of an OTP supervisor |
| Zero-downtime runtime-process addition | **yes** | New warm workers start dynamically inside the running BEAM VM |
| Zero-downtime service rollout | **yes** | Two replicas, readiness drain, `maxUnavailable: 0`, PDB, longest-invoke grace |
| Compile or syntax check before activation | **yes** | Runtime-aware `/check`, isolated in the target container when required |
| Durable multi-step workflows | **yes** | Persisted runs and steps, retries, sleep/wait, external signals, cancel |
| Browser automation | **yes** | Hardened Chromium with Playwright and Puppeteer plus SSRF/robots policy |
| Event bus invocation | **partial** | NATS subjects and queue group; needs CloudEvents envelopes and filter bindings |
| Stateful actor / durable object | **partial** | Reusable per-key BEAM processes; needs transactional per-key durable storage |
| Metrics, logs, and traces | **yes** | Prometheus, structured stdout, and OTLP |
| Runtime secrets and environment | **yes** | Secret-backed runner config; child environments are explicitly minimized |
| Network isolation | **partial** | Hardened containers and Kubernetes NetworkPolicy; needs per-function egress policy |
| Function revisions | **gap** | Immutable code/config snapshots with stable IDs |
| Aliases and weighted traffic | **gap** | Named aliases, canary percentages, affinity, instant rollback |
| Asynchronous invocation queue | **partial** | Postgres-durable 202 acceptance, per-function idempotency, cross-replica leases, attempt history, status, cancellation, crash recovery, and maximum event age; retention policy and native event-source ack/replay remain |
| Retry policy and destinations / DLQ | **partial** | Per-invocation bounded exponential retry plus success/failure/canceled/DLQ NATS subjects; destination publish is best effort until JetStream-backed |
| Queue batching and partial acknowledgements | **gap** | Batch size/window, per-message ack/retry, visibility timeout |
| Scheduled / cron triggers | **gap** | UTC cron discovery, overlap policy, retries, history |
| CloudEvents trigger bindings and filters | **gap** | HTTP/NATS sources, attribute filters, fan-out, authenticated sinks |
| Response streaming | **gap** | Backpressure-aware chunked/SSE response protocol |
| WebSockets | **gap** | Supervised connection actors, hibernation/persistence strategy |
| Per-function concurrency controls | **partial** | Bounded per-replica worker pools, single-flight affinity keys, immediate HTTP 429 backpressure, safe lease cleanup, and busy/idle/rejection metrics; fleet-wide reservations and durable overflow remain |
| Scale-to-zero / autoscale | **partial** | Kubernetes deployment today; needs KEDA/Knative-style demand scaling |
| Edge or multi-region placement | **gap** | Region policy, data locality, replicated routing, failover |
| Layers / shared dependency bundles | **gap** | Immutable digest-addressed dependency layers |
| Code signing / provenance policy | **gap** | Signature verification, SBOM, admission policy, attestation |
| Background work after response | **gap** | Bounded `waitUntil`-style supervised task lifecycle |
| Jobs / map fan-out | **partial** | Workflow activities exist; needs first-class parallel jobs and result reduction |

## What the reference platforms teach us

AWS contributes the strongest function lifecycle vocabulary: immutable versions,
aliases, concurrency controls, event-source mappings, async retries and
destinations, layers/extensions, streaming, and long-lived durable functions.
Google contributes buildpack-to-container portability, Eventarc/CloudEvents,
jobs, worker pools, revision traffic splitting, min/max instances, and VPC
integration. Cloudflare contributes globally placed isolates, bindings, Queues,
Durable Objects, WebSockets, Workflows, cron, and version affinity.

Azure reinforces triggers/bindings, deployment slots, identities, and durable
orchestration/entities. Vercel contributes deployment immutability, preview
deployments, skew protection, fluid per-instance concurrency, streaming, and
bounded background work. Deno contributes code-discovered cron, instant
rollback, integrated builds, telemetry, caching, and self-hostable regions.
Knative contributes the best self-hosted model for immutable revisions,
CloudEvents routing, scale-to-zero, and Kubernetes-native traffic splitting.

## Implementation waves

### Wave 1 — BEAM-native availability

- Put every warm runtime process under an OTP supervision tree.
- Keep failures local to one worker while rebuilding the manager and worker
  generation coherently if either supervisory component fails.
- Separate liveness from readiness and drain new traffic before termination.
- Guarantee at least one ready pod through rollout and voluntary disruption.
- Expose authenticated, source-free runtime-process state and Prometheus gauges.

### Wave 2 — reliable asynchronous execution

- Bound local per-function concurrency with exclusive worker leases, immediate
  overload rejection, abandoned-request cleanup, and old-generation draining.
- Persist async invocations through managed one-activity workflows with
  caller-supplied idempotency keys, status, attempt history, and cancellation.
- Enforce bounded retry, backoff, per-attempt timeout, and maximum event age;
  emit terminal success/failure/canceled and DLQ events.
- Support sync, async, and stream invocation modes.
- Make destinations JetStream-durable and add configurable retry jitter and
  retention.
- Add queue consumer batching, visibility leases, partial acknowledgement, and
  concurrency/backpressure controls.

### Wave 3 — revisions and safe delivery

- Snapshot function code, runtime, env references, resource policy, and trigger
  configuration into immutable revisions.
- Add aliases with weighted routing, version affinity, 0% preview URLs, canary
  metrics, promotion, abort, and instant rollback.
- Keep in-flight requests on their selected revision while new requests move.
- Add signature, provenance, SBOM, and image-digest policy.

### Wave 4 — universal triggers and state

- Adopt CloudEvents as the canonical external event envelope.
- Add cron, HTTP, NATS, queue, webhook, database-change, and object-change
  bindings with attribute filters and fan-out.
- Add durable keyed actors with transactional storage, alarms, and supervised
  WebSocket sessions.
- Add service bindings and per-function identity/egress policy.

### Wave 5 — placement and elasticity

- Extend local per-function max concurrency to fleet-wide min/max capacity and
  reservations.
- Add demand-based autoscaling and scale-to-zero without losing durable queues.
- Add region/data-locality policy, health-aware failover, and replicated routing.
- Preserve the same control-plane contract for single-node, Kubernetes, and
  multi-region installations.

## Non-negotiable BEAM rule

Trusted platform capabilities should be ordinary supervised Gleam/Erlang
processes that can be started while the node is running. Untrusted customer
function code remains behind the existing child-process or hardened-container
boundary. Arbitrary uploaded BEAM modules must never be loaded into the server
VM merely to claim hot-code support; live module upgrades require verified
artifacts, an operator-controlled allowlist, explicit state migration, draining,
health checks, and rollback.
