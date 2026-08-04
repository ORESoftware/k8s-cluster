# GitHub Actions continuity: ARC parity plus an independent mirror

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
