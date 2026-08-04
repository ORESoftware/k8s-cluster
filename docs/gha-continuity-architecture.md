# GitHub Actions continuity: ARC parity plus an independent mirror

Linear: DEN-1549, DEN-1550, DEN-1597  
Last reviewed: 2026-08-04

## Executive summary

CI continuity is implemented as cooperating lanes rather than a claim that one
custom service can reproduce GitHub's proprietary control plane.

1. **GitHub-hosted Actions** remain the normal bootstrap, comparison, native
   macOS/Windows, signing, and emergency lane while allocation is available.
2. **Official Actions Runner Controller (ARC)** preserves native GitHub Actions
   workflow semantics while placing ephemeral trusted Linux runners in AWS and
   Hetzner.
3. **`gha-clone-server-rs`** provides an independent, bounded workflow
   compatibility lane. It compiles a strict static subset to fixed profiles on
   the existing build server.
4. **`gha-executor-router`** selects a reviewed independent executor at a safe
   pre-submit boundary and pins every accepted build to that provider.
5. **`dd-build-server`** remains the only fixed-profile executor and retains job,
   artifact, Postgres, NATS, container-build, and Fiducia responsibilities.
6. **`gha-capacity-broker-rs`** owns billing-aware hosted-versus-ARC policy. It
   does not parse workflows, choose independent executor URLs, or execute code.

The code implementation is reviewed in
[`ORESoftware/k8s-cluster#645`](https://github.com/ORESoftware/k8s-cluster/pull/645).
The inert GitOps wiring is reviewed separately in
[`ORESoftware/k8s-cluster#650`](https://github.com/ORESoftware/k8s-cluster/pull/650).
Both clone-server and router Deployments remain at `replicas: 0`, with API,
webhook, and router execution disabled.

## Current hosted-capacity evidence

Fresh August 4, 2026 workflows acquired real GitHub-hosted Ubuntu 24.04 workers
and executed checkout, Rust, Node, repository-contract, and process-test steps.
Hosted Actions are therefore allocating for `ORESoftware/k8s-cluster` at the time
of this review.

That evidence does **not** reveal the exact current organization included-minute
balance, spending limit, or billing-period usage. Numeric usage remains the
responsibility of the dedicated billing-read GitHub App and its
`/usage/summary` contract from DEN-1549. A job reaching a hosted runner proves
allocation; it is not a substitute for billing evidence.

The capacity broker therefore fails closed:

- use hosted capacity while policy permits;
- move opted-in trusted Linux jobs to certified ARC when thresholds require it;
- signal an approved fixed-profile path only when native capacity is unavailable
  and that profile has independent certification; and
- hold rather than invent a runner label when neither certified lane is ready.

## Lane A: native parity through ARC

Official ARC keeps GitHub's workflow parser, expressions, matrices, marketplace
actions, checks, artifacts, caches, environments, OIDC, reusable workflows, and
branch-protection identity. It changes runner placement, not workflow semantics.

The repository contains source-complete, activation-gated AWS and Hetzner ARC
Applications using chart `0.14.2` and runner `2.334.0`:

| Scale-set label | Provider/group | Capability | Isolation |
| --- | --- | --- | --- |
| `sonus-ci` | AWS / `sonus-aws` | Rust, Node, Python, Dart/Flutter analysis, tests, web and Linux | non-privileged one-job pods |
| `sonus-ci` | Hetzner / `sonus-hetzner` | same certified trusted-Linux class | non-privileged one-job pods |
| `sonus-browser` | provider-specific | Chromium Playwright/Puppeteer | non-privileged browser image |
| `sonus-ci-dind` | AWS initially | trusted service containers and image builds | separately reviewed DinD sidecar; no host socket |
| `sonus-android-kvm` | dedicated KVM nodes | Android emulator | tainted node pool, quota, and admission policy |

The scale sets start with zero warm runners and bounded maximums. macOS/iOS and
Windows native builds remain GitHub-hosted or require future dedicated native
machines.

ARC activation still requires:

- a narrowly scoped runner-registration GitHub App through External Secrets;
- runner groups and repository access configured in GitHub;
- controller and CRD ownership audit;
- immutable digest-pinned runner images with SBOM and provenance;
- provider-specific read-only smokes;
- hosted-versus-AWS-versus-Hetzner result comparison; and
- a practiced rollback to zero scale.

A pasted classic personal access token is not an ARC registration credential.

## Lane B: independent workflow compatibility

`remote/deployments/gha-clone-server-rs` is a Rust parser, planner, run store,
and dispatcher for a deliberately bounded static workflow subset. It exists for
periods when the GitHub runner/control-plane path is unavailable or
cost-constrained.

The service:

1. accepts an authenticated request or a verified GitHub webhook;
2. reads only exact allowlisted repositories and workflow paths;
3. requires an immutable lowercase 40-hex commit;
4. parses bounded workflow YAML;
5. validates job IDs, static `needs`, cycles, runner evidence, expressions,
   actions, secrets, containers, and size/runtime limits;
6. reports native ARC compatibility separately from independent support;
7. maps recognized jobs to fixed build-server profiles; and
8. submits only the canonical repository URL, immutable commit, fixed profile,
   and deterministic request identity.

The independent lane rejects dynamic matrices, reusable workflows, conditions,
secret or OIDC expressions, arbitrary marketplace actions, service/job
containers, environments, deployments, native macOS/Windows execution,
caller-selected shell, image, Dockerfile, context, working directory, headers,
URLs, or Kubernetes objects.

A job may be ARC-compatible and independently unsupported. That distinction is a
security feature and remains visible in every plan.

### Fixed-profile classes

Current reviewed mappings include:

| Workflow evidence | Fixed profile |
| --- | --- |
| Cargo formatting, Clippy and tests | `rust-verify` |
| generic Node install and tests | `node-verify` |
| lifecycle-suppressed install, operator checks and focused tests | `node-hardened-verify` |
| lifecycle-suppressed install and complete tests | `node-hardened-test` |
| Python compile and pytest | `python-verify` |
| Flutter analyze and tests | `flutter-verify` |
| Flutter Android debug build | `flutter-android-debug` |
| Flutter web release | `flutter-web-release` |
| Flutter Linux release | `flutter-linux-release` |
| Playwright | `playwright` |
| Puppeteer | `puppeteer` |

The compiler does not forward source workflow commands into the executor.

## Component authority boundaries

| Component | Authority |
| --- | --- |
| GitHub Actions | native orchestration and hosted runner allocation |
| ARC | ephemeral self-hosted placement while retaining native semantics |
| `gha-capacity-broker-rs` | billing/capacity evidence and hosted-versus-ARC routing variables |
| `gha-clone-server-rs` | bounded workflow parsing, planning, run state and fixed-profile dispatch |
| `gha-executor-router` | ordered independent executor readiness selection and pinned status routing |
| `dd-build-server` | fixed-profile execution, jobs, artifacts, Postgres/NATS and Fiducia integration |

The clone server does not select a cloud. The executor router does not parse
workflow YAML. The build server does not accept arbitrary repository commands
from either service.

## AWS and Hetzner independent routing

`gha-executor-router` preserves the existing authenticated build-server API:

```text
POST /builds
GET /builds/{id}
x-build-server-auth
build-server.v1 / run-profile
```

The router accepts only an exact credential-free GitHub HTTPS origin, immutable
lowercase commit, bounded fixed-profile slug, deterministic request ID, and no
unknown fields. Callers cannot select a provider, endpoint, credential, command,
image, action implementation, or Kubernetes object.

AWS is the first independent executor because the reviewed build server and its
persistence already exist in the cluster. Hetzner is represented by a separate,
disabled provider identity. It has no endpoint or credential until its own
fixed-profile service, TLS identity, shared artifact policy, and provider-loss
evidence exist.

### No-duplicate pre-submit rule

Provider selection may advance from AWS to Hetzner only at the **pre-submit**
readiness boundary.

1. The router probes configured executors in reviewed order.
2. It may skip an executor whose bounded `/readyz` probe is unavailable.
3. It submits the immutable request to the first ready executor.
4. After any `POST /builds` attempt, automatic cross-provider submission stops.

A connection reset, timeout, HTTP 429, HTTP 5xx, redirect, oversized body,
malformed HTTP 202 response, or other uncertain outcome after POST is ambiguous:
the first executor may already have persisted or started the request. The router
fails closed rather than risking duplicate execution.

An explicit HTTP 4xx fixed-contract rejection also fails closed. A valid HTTP
202 response produces a namespaced `<executor>~<upstream-id>` route. Every
status read remains pinned to that executor; polling failure never switches
providers or creates a second job.

Cross-provider resumption after an attempted POST requires one shared durable
job and artifact model plus a Fiducia-fenced authoritative assignment. Version
1 does not pretend readiness probing provides that transaction.

## Deterministic request idempotency

Merged PR
[`ORESoftware/k8s-cluster#643`](https://github.com/ORESoftware/k8s-cluster/pull/643)
made `dd-build-server` deterministic request handling safe within one process:

- identical request IDs and immutable payloads reattach to one retained job;
- the same ID with changed execution inputs returns a conflict;
- queue-full rejection does not consume the identity;
- concurrent duplicate admission creates one job; and
- pruning a terminal job releases its retained identity.

Restart-durable and cross-replica idempotency remain a Postgres/Fiducia
follow-up. The router forwards the deterministic request ID unchanged; it does
not claim that forwarding alone creates durable idempotency.

## Webhook fallback boundary

The `workflow_run` fallback is a bounded trigger, not an unconstrained second
runner. It dispatches only when:

- raw-body `X-Hub-Signature-256` HMAC is valid;
- `X-GitHub-Delivery` is a valid UUID;
- repository and workflow path are exactly allowlisted;
- action is `completed` and conclusion is in the reviewed failure set;
- the immutable workflow at `head_sha` passes the bounded compiler;
- every job maps to a fixed profile;
- recursion exclusions pass; and
- the delivery has not already claimed an independent dispatch.

Delivery retention is bounded by TTL and entry count. Single-process retention
supports one active replica. Horizontal webhook execution requires a shared
durable or Fiducia-fenced claim before replicas can increase.

## Security contract

- exact `aws` and `hetzner` provider enum;
- bounded lowercase executor IDs and unique enabled origins;
- disabled executors omit endpoint and credential state;
- credentials are read from direct-child files beneath one absolute secret root;
- mounted credentials are bounded single-line values without NUL, CR, or LF;
- inbound router auth is distinct from executor auth;
- the AWS router mount reuses the existing
  `dd-agent-secrets.SERVER_AUTH_SECRET` authority instead of copying it into a
  second backing secret;
- Hetzner receives a distinct credential only when enabled;
- production cross-cloud endpoints require HTTPS;
- HTTP is limited to loopback tests and exact Kubernetes Service DNS;
- URL credentials, paths, query strings, fragments, and redirects are rejected;
- the HTTP client follows no redirects;
- request and upstream bodies, timeouts, errors, executor count, and metric
  labels are bounded;
- authentication uses constant-time digest comparison;
- upstream response bodies and secret values are never reflected in errors,
  health, capabilities, or metrics;
- pods have no service-account token, host path, host runtime socket,
  kubeconfig, or cloud execution credential; and
- execution is disabled by default.

## Inert GitOps state

PR #650 stages ConfigMap, ExternalSecret, Deployment, Service, NetworkPolicy,
clone-server routing, static contracts, and operator documentation in
`dd-next-runtime`.

Both services remain at `replicas: 0`; clone-server API execution, webhook
execution, and router execution remain false.

The zero-replica review scaffold pins clone-server and router source to tested
commit `d1e97bfe45054b3b6329398c2c0787cb0d250622`, fetches that exact SHA without a
branch, checks out detached `FETCH_HEAD`, verifies `rev-parse HEAD`, uses the
committed Cargo lock, and names each Rust binary explicitly.

The router NetworkPolicy permits only:

- clone-server ingress on TCP 8126;
- cluster DNS; and
- the in-cluster AWS `dd-build-server` on TCP 8100.

There is no public 443 or `0.0.0.0/0` egress while Hetzner is disabled. A later
Hetzner activation must add one exact reviewed TLS or private-network route,
never generic Internet access.

The temporary Rust source-build images are acceptable only because replicas
remain zero. Activation requires immutable digest-pinned runtime images with
SBOM, provenance, and vulnerability evidence.

## Test evidence

The exact router head has been exercised on hosted Ubuntu 24.04 with pinned Rust
1.90. Permanent read-only CI runs:

- formatting;
- warnings-as-errors Clippy across all targets;
- every unit and integration target;
- the real compiled router plus two Axum build-server doubles;
- fixed-profile registry and meta fallback tests;
- build-server deterministic request/idempotency tests;
- actionlint;
- GitOps and security contracts; and
- credential-pattern rejection.

The router process suite proves:

- AWS-first acceptance without a Hetzner POST;
- unavailable AWS readiness selecting Hetzner before submission;
- HTTP 4xx rejection without fallback;
- ambiguous post-attempt failure without fallback or body leakage;
- accepted AWS status remaining pinned after polling failure;
- exact request-ID and provider-auth forwarding;
- disabled execution and unauthenticated request rejection;
- redirect refusal;
- malformed duplicate configuration exiting before bind;
- multiline inbound and executor secret rejection before bind; and
- source-redacted startup errors, API errors, health, capabilities, and metrics.

GitOps tests render the complete Kustomize overlay and fail if replicas increase,
execution is enabled, source becomes mutable, a binary is ambiguous, credentials
move inline, a duplicate executor authority appears, direct clone-to-build
egress returns, a dormant Hetzner endpoint or Internet route appears, Argo
resources disappear, or credential markers enter rendered manifests.

## Activation sequence

1. Merge exact router code after permanent read-only checks are
   terminal-successful.
2. Merge the inert GitOps follow-up; keep replicas zero and execution false.
3. Revoke exposed classic tokens and provision separate least-privilege GitHub
   Apps for ARC registration, billing read, workflow read, and approved
   repository-variable mutations.
4. Build and digest-pin ARC runner, clone server, executor router, build server,
   and capacity broker images with SBOM and provenance.
5. Create AWS and Hetzner runner groups and reconcile ARC ExternalSecrets.
6. Register bounded scale sets with zero warm runners and run provider-specific
   read-only smokes.
7. Compare representative Rust, Node, browser, Flutter, artifact, and check
   results across hosted and ARC lanes.
8. Scale clone server and router to one for plan-only and readiness tests while
   execution remains false.
9. Enable one exact AWS fixed-profile smoke and prove deterministic
   reattachment.
10. Provision the Hetzner fixed-profile executor, TLS identity, distinct
    credential, shared artifact policy, and provider-addressable observability.
11. Prove AWS-unready-before-submit selects Hetzner and
    AWS-accepted-then-partitioned remains pinned to AWS without duplicate
    submission.
12. Enable webhook execution only after HMAC, redelivery, recursion,
    idempotency, rollback, and single-replica claim evidence.
13. Keep positive hosted budget for native platforms and emergency fallback.

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
