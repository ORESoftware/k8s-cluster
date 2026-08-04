# GitHub Actions continuity: ARC parity plus an independent mirror

Linear: DEN-1550, DEN-1549, DEN-1597  
Last reviewed: 2026-08-04

## Current capacity evidence

Fresh August 4, 2026 workflows in `ORESoftware/k8s-cluster` acquired real
GitHub-hosted Ubuntu workers and executed setup, Rust, Node, and repository
contract steps. Hosted Actions are therefore allocating for this repository at
the time of review.

That evidence does **not** reveal the exact organization included-minute balance,
spending limit, or billing-period usage. Numeric usage remains the responsibility
of the dedicated billing-read GitHub App and its `/usage/summary` contract from
DEN-1549. A job reaching a hosted runner proves allocation, not an exact balance.

## Honest parity boundary

A custom server cannot truthfully reproduce every proprietary GitHub Actions
feature. Continuity is split into two lanes with different authorities.

### Lane A: native parity through ARC

Official Actions Runner Controller keeps GitHub's workflow parser, expressions,
matrices, marketplace actions, checks, caches, artifacts, environments, OIDC,
reusable workflows, and branch-protection identity. It places ephemeral trusted
Linux workers in AWS and Hetzner.

Planned labels:

| Label | Capability | Isolation |
| --- | --- | --- |
| `sonus-ci` | Rust, Node, Python, Dart/Flutter analysis, tests, web and Linux | non-privileged one-job pod |
| `sonus-browser` | Chromium Playwright/Puppeteer | non-privileged browser image |
| `sonus-ci-dind` | trusted service containers and image builds | separately reviewed DinD sidecar; no host socket |
| `sonus-android-kvm` | Android emulator | dedicated KVM node pool, taints, quota and admission policy |

macOS/iOS and Windows native builds remain GitHub-hosted or use future dedicated
native machines. ARC registration uses a narrowly scoped GitHub App through
External Secrets, never a pasted classic PAT.

### Lane B: independent workflow compatibility

`gha-clone-server-rs` compiles a deliberately bounded static workflow subset to
reviewed fixed profiles on `dd-build-server`. It exists for periods when hosted
allocation, organization budget, or the GitHub runner path is constrained.

The independent lane accepts only:

- exact allowlisted repositories and workflow paths;
- immutable lowercase 40-hex commits;
- static acyclic `needs` graphs;
- supported Linux runner evidence;
- fixed operator-reviewed build profiles; and
- bounded requests, plans, logs, polling, runtime, and retained state.

It rejects dynamic matrices, reusable workflows, conditions, secret/OIDC
expressions, arbitrary marketplace actions, service/job containers, mutable
refs, non-Linux native execution, environments, deployments, caller-selected
commands, images, Dockerfiles, contexts, manifests, headers, URLs, or Kubernetes
objects.

A job may be ARC-compatible while independently unsupported. That distinction is
preserved in every plan rather than approximated.

## Component authorities

| Component | Authority |
| --- | --- |
| GitHub Actions | native workflow orchestration and hosted runner allocation |
| ARC | ephemeral self-hosted runner placement while retaining native semantics |
| `gha-capacity-broker-rs` | hosted-versus-ARC policy and billing/capacity evidence |
| `gha-clone-server-rs` | bounded workflow parsing, planning, run state and fixed-profile dispatch |
| `gha-executor-router` | ordered independent executor readiness selection and provider-pinned status routing |
| `dd-build-server` | fixed command/profile execution, jobs, artifacts, Postgres/NATS and Fiducia integration |

The clone server does not choose a cloud. The executor router does not parse
workflow YAML. The build server does not accept arbitrary commands from either
service.

## Independent AWS/Hetzner routing

The router preserves the existing authenticated build-server API:

```text
POST /builds
GET /builds/{id}
x-build-server-auth
build-server.v1 / run-profile
```

AWS is the first independent executor because the reviewed build server and its
supporting persistence already exist in the cluster. Hetzner is a separately
identified secondary provider and remains disabled until its fixed-profile
endpoint, TLS identity, credential, artifact policy, and provider-loss evidence
exist.

### No-duplicate rule

Automatic AWS-to-Hetzner selection is allowed only at the **pre-submit**
readiness boundary.

1. The router probes reviewed executors in configured order.
2. It may skip an executor whose bounded `/readyz` probe is already unavailable.
3. It submits the immutable fixed-profile request to the first ready executor.
4. After any `POST /builds` attempt, automatic cross-provider submission stops.

A connection reset, timeout, HTTP 429, HTTP 5xx, redirect, oversized body,
malformed acceptance response, or other uncertain result after POST is ambiguous:
the first executor may already have persisted or started the deterministic
request. The router fails closed rather than risking duplicate execution.

An explicit HTTP 4xx contract rejection also fails closed. HTTP 202 acceptance
produces a namespaced `<executor>~<upstream-id>` route. Every status read remains
pinned to that executor; polling failure never creates a second job.

Cross-provider resumption after an attempted POST requires a shared durable job
and artifact model plus one Fiducia-fenced authoritative assignment. Version 1
does not pretend readiness probing provides that transaction.

## Idempotency boundary

Merged PR #643 made `dd-build-server` deterministic-request handling safe inside
one process:

- identical request IDs and immutable payloads reattach to one retained job;
- the same ID with changed execution inputs returns a conflict;
- queue-full rejection does not consume the identity;
- concurrent duplicate admission creates one job; and
- pruning a terminal job releases its retained identity.

Restart-durable and cross-replica identity remains a Postgres/Fiducia follow-up.
The router forwards the deterministic request ID unchanged; it does not claim
that forwarding alone creates durable idempotency.

## Webhook fallback boundary

The `workflow_run` fallback is not an unconstrained second runner. It dispatches
only when all of these hold:

- raw-body `X-Hub-Signature-256` HMAC is valid;
- `X-GitHub-Delivery` is a valid UUID;
- repository and workflow path are exactly allowlisted;
- action is `completed` and conclusion is in the reviewed failure set;
- the immutable workflow at `head_sha` passes the bounded compiler;
- every job maps to a fixed profile;
- recursion exclusions pass; and
- the delivery has not already claimed an independent dispatch.

Single-process delivery retention is bounded by TTL and entry count. Horizontal
webhook execution requires a shared durable or Fiducia-fenced claim before more
than one replica is allowed.

## Security contract

- credentials are mounted from direct-child files under one absolute secret root;
- mounted secrets are bounded single-line values without NUL, CR, or LF;
- inbound router auth is distinct from executor auth;
- the AWS router mount reuses the existing `dd-agent-secrets.SERVER_AUTH_SECRET`
  authority instead of copying it into another backing secret;
- Hetzner receives a separate credential only when enabled;
- URL credentials, query strings, fragments, arbitrary paths and redirects are
  rejected;
- cross-cloud endpoints require HTTPS; plain HTTP is limited to loopback and
  exact Kubernetes Service DNS;
- the HTTP client follows no redirects;
- error bodies and metrics are source-redacted and bounded;
- pods have no service-account token, host socket, host path, kubeconfig, or
  cloud execution credential; and
- execution is disabled by default.

## Inert GitOps state

PR #650 stages the router in `dd-next-runtime` while keeping both continuity
services at `replicas: 0` and both execution flags false.

The review scaffold pins clone-server and router source to tested commit
`6146668400441de15a8d8e9f513786096db9a730`, fetches that exact SHA without a
branch, checks out detached `FETCH_HEAD`, verifies `rev-parse HEAD`, uses the
committed Cargo lock, and names each binary explicitly.

The router NetworkPolicy admits only:

- clone-server ingress on 8126;
- cluster DNS; and
- the in-cluster AWS `dd-build-server` on 8100.

There is no dormant public 443 or `0.0.0.0/0` egress while Hetzner is disabled.
A later Hetzner activation must add one exact reviewed TLS/private-network
route, not generic Internet access.

The temporary Rust source-build containers are acceptable only because replicas
remain zero. Activation requires immutable digest-pinned runtime images with
SBOM, provenance, and vulnerability evidence.

## Test evidence

The router suite uses the real compiled process plus two local Axum build-server
doubles. It covers:

- AWS-first acceptance without a Hetzner POST;
- failed AWS readiness selecting Hetzner before submission;
- HTTP 4xx rejection without fallback;
- ambiguous post-attempt HTTP 5xx without fallback or response-body leakage;
- accepted AWS status remaining pinned after polling failure;
- exact request-ID and provider-auth forwarding;
- disabled execution, authentication, immutable request, and unknown-field
  boundaries;
- malformed duplicate configuration exiting before bind;
- redirect refusal;
- multiline inbound and executor secret rejection before bind; and
- no secret values in startup errors, health, capabilities, metrics, or API
  responses.

The pinned Rust 1.90 validation runs formatting, `cargo check --all-targets`,
locked warnings-as-errors Clippy, and every unit, binary, HTTP, webhook, meta,
adversarial, startup, and dual-provider process target. GitOps tests render the
complete Kustomize overlay and verify source pins, projected secrets, zero
replicas, disabled execution, exact network paths, Argo inclusion, and absence of
credential markers.

## Activation sequence

1. Merge the webhook hardening and exact router code after their permanent
   read-only checks are terminal-successful.
2. Merge the inert GitOps child; keep replicas zero and execution false.
3. Revoke exposed classic PATs and provision separate least-privilege GitHub Apps
   for ARC registration, billing read, workflow read, and approved mutations.
4. Build and digest-pin ARC runner, clone server, executor router, build server,
   and capacity broker images with SBOM and provenance.
5. Register bounded AWS and Hetzner ARC scale sets with zero warm runners and run
   provider-specific read-only smokes.
6. Compare representative Rust, Node, browser, Flutter and artifact/check results
   across hosted and ARC lanes.
7. Scale clone server and router to one for plan-only and readiness tests while
   execution remains false.
8. Enable one exact AWS fixed-profile smoke and prove deterministic reattachment.
9. Provision the Hetzner fixed-profile executor, certificate, credential, shared
   artifact policy, and provider-addressable observability.
10. Prove AWS-unready-before-submit selects Hetzner and AWS-accepted-then-
    partitioned remains pinned to AWS without duplicate submission.
11. Enable webhook execution only after HMAC, redelivery, recursion, idempotency,
    rollback, and single-replica claim evidence.
12. Keep positive hosted budget for native platforms and emergency fallback.

## Rollback

- set ARC maxima to zero or restore hosted routing labels;
- set clone-server and router execution flags false and replicas to zero;
- restore clone-server routing directly to the reviewed AWS build server before
  removing router configuration;
- disable Hetzner and remove its egress before retiring its credential;
- retain `dd-build-server` independently for approved fixed profiles; and
- never bypass a required check because a continuity lane is unavailable.

## Repository boundary

The implementation remains in `ORESoftware/k8s-cluster` while parser, router,
profiles, manifests, policy contracts, and rollback are reviewed together.
Extraction to separate protected repositories is a later bootstrap operation,
not a prerequisite for the inert or ARC lanes.
