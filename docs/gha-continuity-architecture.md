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

The service records a delivery claim only after workflow retrieval, planning,
and execution readiness succeed. Transient GitHub/API failures therefore remain
retryable using the original delivery ID. A write lock serializes concurrent
copies inside one process so only one copy can dispatch.

Delivery retention is bounded by a nonzero TTL and entry cap. It is initially an
in-memory map, which is sufficient only while the deployment uses one replica
and the `Recreate` strategy. Horizontal webhook execution requires a shared
durable claim store or a Fiducia-fenced authoritative claim. Scaling replicas
without that shared claim would weaken the duplicate-delivery guarantee and is
therefore outside the activation contract.

The GitHub workflow-read origin defaults to `https://api.github.com` and is
explicitly declared in GitOps. Configuration rejects credentials, query strings,
fragments, and non-HTTPS origins; plain HTTP is accepted only for loopback test
servers so the complete fetch path can be exercised hermetically.
Linear: DEN-1550  
Last reviewed: 2026-08-03

## Confirmed incident state

The Sonus Auris organization exhausted its July 2026 included GitHub-hosted
Actions allowance. On August 3, after the expected monthly reset, failed
workflow run `30642692260` was explicitly rerun. The new jobs
`91768871098`, `91768871113`, `91768871116`, and `91768871157` again failed
before runner setup and exposed neither steps nor logs.

This confirms a continuing organization-level runner allocation problem. It
does not prove whether the current cause is included-minute exhaustion, a zero
spending limit, payment policy, disabled Actions, or another billing/control
restriction. Exact remaining minutes and administrative policy are visible only
in GitHub billing and organization settings; do not infer the balance from a
pre-runner failure alone.

## Architecture

A custom service cannot honestly reproduce every proprietary GitHub Actions
feature. Continuity is therefore split into two lanes.

### Lane A: native parity through ARC

Actions Runner Controller provides ephemeral self-hosted runners while GitHub
continues to own workflow parsing, expressions, matrices, marketplace actions,
checks, artifacts, caches, environments, OIDC, and branch-protection identity.

Planned labels:

| Label | Capability | Isolation |
| --- | --- | --- |
| `sonus-ci` | Rust, Node, Python, Dart/Flutter analysis/tests/web/Linux | non-privileged ephemeral pod |
| `sonus-browser` | Chromium Playwright/Puppeteer | non-privileged browser pod |
| `sonus-ci-dind` | trusted service containers and image builds | separate privileged DinD sidecar, never host Docker socket |
| `sonus-android-kvm` | Android emulator | dedicated KVM node pool and admission policy |

macOS/iOS and Windows native builds remain on GitHub-hosted runners or future
dedicated native machines.

The existing reviewed scaffold is under
`remote/argocd/ci-runners/sonus-auris/`. Activation still requires an
immutable runner image digest and a least-privilege GitHub App reconciled
through External Secrets.

### Lane B: independent workflow compatibility

`remote/deployments/gha-clone-server-rs` parses a bounded static workflow
subset and compiles supported jobs to fixed profiles on the existing
`dd-build-server` API.

The service never sends caller-selected shell or images to the executor. It
submits only:

- exact allowlisted `owner/repo`;
- immutable 40-hex commit SHA;
- fixed profile name;
- deterministic idempotency identity.

Supported profile classes initially cover Rust, Node, Python, Flutter,
Playwright and Puppeteer. Static `needs` edges are validated and executed in
deterministic topological order.

The planner returns separate fields for:

- native ARC compatibility and required lane;
- independent-lane support;
- fixed profile, when supported;
- exact reasons for every rejected independent behavior.

### Independent executor routing

`src/bin/executor_router.rs` is a stateless signed router between the mirror and
one or more reviewed `dd-build-server` executors. It makes the independent lane
multi-cloud without giving the workflow parser cloud credentials or direct
executor topology.

The Kubernetes pod routes the planner to `http://127.0.0.1:8126`. The router
then selects an AWS or Hetzner executor by fixed-profile support, numeric
priority and stable route name. Route inventory and per-provider authentication
are ExternalSecret values, not committed endpoints or credentials.

Failover is intentionally narrow:

- retry another route only after a connection failure before a response, or an
  explicit `429`, `502`, `503` or `504`;
- never retry after `202 Accepted`;
- never retry an ambiguous timeout after a connection may have delivered the
  request;
- never retry validation, authentication, authorization or other fatal HTTP
  responses.

Accepted upstream job IDs are wrapped in HMAC-SHA256 signed route tokens.
Status polling therefore returns to the exact executor that accepted the job
without storing an in-memory routing table. See `docs/gha-executor-router.md`
for the configuration and activation contract.

## Fail-closed boundary

The independent lane rejects dynamic matrices, reusable workflows, conditions,
secret/OIDC expressions, arbitrary marketplace actions, service/job containers,
macOS/Windows execution, environments, deployments, caller-selected commands,
and mutable branch/tag execution.

The executor router further rejects non-profile jobs, mutable revisions,
missing idempotency identities, unsupported profiles, malformed route
inventories and unsigned status identifiers.

A job can be supported by ARC while rejected by the independent mirror. This is
a normal and preserved distinction, not a failure to report.

## Existing build server integration

`dd-build-server` remains the executor and owns its existing containerd,
BuildKit, ECR, artifact, NATS, Postgres, and Fiducia boundaries. DEN-1550 adds
fixed `rust-verify`, `node-verify`, and `python-verify` profiles beside the
existing Flutter and browser profiles.

`gha-clone-server-rs` and its router have no Kubernetes service-account token,
hostPath, Docker socket, containerd socket, BuildKit socket, cloud credentials,
or production kubeconfig.

The router does not make executor-side idempotency optional. Every submission
includes the deterministic `requestId`; persistent reconciliation and
Fiducia-fenced claims are still required before multiple independent executors
are authoritative concurrently.

## AWS and Hetzner

The GitOps resources are part of the shared `dd-next-runtime` render and
therefore can exist in AWS and Hetzner. The deployment begins at zero replicas.

- AWS is the initial independent execution authority because `dd-build-server`,
  BuildKit/containerd, ECR and its persistence already exist there.
- Hetzner can host non-privileged ARC scale sets immediately.
- The signed router can target a Hetzner fixed-profile executor after its
  reviewed endpoint and scoped secret are provisioned.
- A second authoritative independent executor still requires shared artifact
  storage, durable request-ID reconciliation and Fiducia-fenced claims.

Provider order is policy, not code: `GHA_EXECUTOR_ROUTES_JSON` supplies route
priorities and profile allowlists. The intended initial policy is AWS primary
and Hetzner secondary.

## Secret contract

Backing secret: `dd/remote-dev/gha-clone-server-secrets`

Required properties:

- `auth_secret`
- `github_webhook_secret`
- `github_app_installation_token`
- `build_server_auth`
- `executor_routing_secret`
- `executor_routes_json`
- `hetzner_build_server_auth`

`github_app_installation_token` must be produced by the approved GitHub App
broker and rotated before expiry. Do not store a classic PAT in the backing
secret.

`executor_routes_json` contains only route metadata and `authEnv` references;
it must never embed credentials. `executor_routing_secret` signs stateless
route IDs and must be independently random. Provider build-server credentials
remain separate secret properties.

## Activation

1. Merge source, profiles, tests, workflow, router and GitOps resources.
2. Provision and validate the ExternalSecret without displaying values.
3. Build and digest-pin the ARC runner image.
4. Register `sonus-ci` with `minRunners: 0` and a bounded maximum.
5. Run the opt-in `arc-parity-smoke` workflow.
6. Scale `dd-gha-clone-server` from zero to one with execution still disabled.
7. Submit plan-only fixtures and compare classifications to the original GHA
   workflows.
8. Enable API execution only for trusted immutable commits and prove the meta
   self-test through the real `dd-build-server`.
9. Register only the `workflow_run` failure webhook through the secret-safe
   registration script.
10. Prove HMAC rejection, exact-path filtering, recursion exclusion, retry after
    transient retrieval failure, concurrent duplicate suppression, and
    build-server idempotency.
11. Enable webhook execution only while the deployment remains single-replica,
    until shared delivery persistence or Fiducia fencing is implemented.
12. Keep positive GitHub-hosted budget for native platforms and emergency use.
8. Enable API execution only for trusted immutable commits.
9. Enable webhook execution only after HMAC, workflow fetch and idempotency
   evidence.
10. Keep positive GitHub-hosted budget for native platforms and emergency use.
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
3. Replace in-pod source compilation with digest-pinned service images.
4. Build and digest-pin the ARC runner image.
5. Register `sonus-ci` with `minRunners: 0` and a bounded maximum.
6. Run the opt-in `arc-parity-smoke` workflow.
7. Keep `dd-gha-clone-server` at zero replicas while validating the route
   inventory and provider secrets in a non-production namespace.
8. Scale the pod to one with both execution flags still disabled.
9. Verify the planner and executor-router readiness endpoints and run plan-only
   fixtures.
10. Submit one immutable fixed-profile smoke to AWS.
11. Prove explicit-capacity failover to Hetzner and prove accepted or invalid
    primary requests never reach Hetzner.
12. Enable API execution only for trusted immutable commits.
13. Enable webhook execution only after HMAC, workflow fetch, delivery
    deduplication and replay evidence.
14. Keep positive GitHub-hosted budget for native platforms and emergency use.

## Rollback

- Set ARC scale-set maximum to zero or remove routing labels.
- Set `dd-gha-clone-server` replicas to zero and disable webhook rules.
- Remove or disable a route only after its accepted jobs are drained, because
  signed job IDs resolve through the named route.
- Do not bypass required checks. Restore hosted capacity or execute a reviewed
  equivalent with retained evidence.

## Repository boundary

No standalone `ORESoftware/gha-clone-server.rs` repository currently exists.
The implementation remains in `ORESoftware/k8s-cluster` so source, executor
profiles, manifests, route policy and contract tests are reviewed atomically.
Extraction is tracked through the protected repository-bootstrap workflow after
the API and golden fixtures stabilize.

## Failure webhook contract

GitHub sends `workflow_run` completion events for every conclusion. The independent
lane therefore accepts only `action=completed`, a configured failure conclusion,
an exact allowlisted workflow path, and a workflow name not present in the loop
exclusion set. A UUID `X-GitHub-Delivery` is inserted into a bounded TTL store only
after workflow retrieval and planning succeed, execution prerequisites are ready,
and every plan is independently executable. Transient fetch, planning, policy, or
readiness errors therefore remain retryable.

Repository hooks are used for repositories owned by the `ORESoftware` user account;
organization hooks are used only for actual GitHub organizations. Registration is
performed with the secret-safe script in `gha-clone-server-rs/scripts` after ingress,
GitHub App permissions, and External Secrets are reconciled. The initial deployment
remains single-replica and disabled; horizontal scaling requires shared delivery
persistence or a fenced claim before activation.
The implementation remains in `ORESoftware/k8s-cluster` while parser, router,
profiles, manifests, policy contracts, and rollback are reviewed together.
Extraction to separate protected repositories is a later bootstrap operation,
not a prerequisite for the inert or ARC lanes.
