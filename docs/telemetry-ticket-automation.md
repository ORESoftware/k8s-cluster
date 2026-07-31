# Telemetry incident automation: foundation and activation boundary

This change lands the secret-free observability foundation for turning sustained backend failures into reviewable remediation work. It deliberately separates signal production from authenticated ticket delivery.

## Merged foundation

The cluster-side foundation:

- enables Loki ruler evaluation for bounded `dd.log.v1` warning and error bursts;
- derives low-cardinality service metrics from OpenTelemetry traces through the collector's span-metrics connector;
- exports OTLP telemetry from the T2V, live-mutex-mills, GCS, and related runtime workloads through explicitly allowed network paths;
- pins the worker-broker model identities used by the 04:00 `America/New_York` remediation lane;
- accepts allowlisted exact-SHA upstream promotion requests and opens draft-only GitOps PRs;
- retains current Prometheus RBAC and static/discovery contracts;
- keeps the coordinator in its protected standalone Argo CD Application.

The current coordinator authority is `ORESoftware/ai-agent-coordinator.rs`. Its app repository owns provider identities, credentials, persistence, workload manifests, and fail-closed delivery behavior. This repository must not recreate `dd-ai-agent-runner` resources inside `dd-next-runtime` or duplicate coordinator Applications.

## Data and safety boundary

Signals must remain low-cardinality and content-minimized. Rules may use service, namespace, deployment, severity, and bounded time windows. Raw log bodies, credentials, customer identifiers, request/task/user IDs, and trace/span IDs must not be copied into ticket prompts or issue bodies.

Upstream promotion is PR-only. It accepts an allowlisted repository and exact commit SHA, advances the corresponding pointer on a feature branch, and opens a draft PR. It never writes to the default branch, merges a PR, or deploys directly.

Promotion uses two deliberately non-interchangeable credentials. The workflow's built-in `GITHUB_TOKEN` may write only a feature branch and draft PR in this GitOps repository. A separately installed GitHub App is minted at runtime with `contents:read` for exactly the selected allowlisted upstream repository; its token is masked, never persisted in a remote URL, and revoked immediately after the gitlink is resolved. No organization PAT is accepted. Missing App credentials or repository installation fail closed before a branch is created.

## Deferred authenticated delivery

Authenticated Alertmanager delivery is intentionally not activated by the foundation merge. A separate reviewed change must add the `dd-alertmanager-telemetry` ExternalSecret and bearer-authenticated receiver only after all of the following are verified:

1. `dd/remote-dev/telemetry-ticket-automation` exists through the approved protected secret path with `TELEMETRY_WEBHOOK_TOKEN`;
2. the standalone coordinator's required ExternalSecrets are Ready;
3. the immutable coordinator Application revision is reviewed and healthy;
4. Alertmanager, Loki ruler, OTEL span metrics, idempotent issue upsert, redaction, retry, and rollback behavior are exercised with sanitized evidence;
5. no automation is permitted to merge generated remediation PRs.

The Prometheus trace-error alert is also deferred to that activation change so the current large rule inventory can be reconciled independently rather than replaced by a stale file side.

## Validation

`telemetry-foundation.yml` renders the affected Kustomize overlays and runs a content-based contract test. The test rejects copied `dd-ai-agent-runner` ownership, verifies Loki rule wiring and low-cardinality constraints, verifies span-metrics and OTLP paths, and confirms the exact-SHA promotion workflow remains least-privilege, owner/repository-scoped, and draft-PR-only.
