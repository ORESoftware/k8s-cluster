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
restriction. Organization billing and Actions budget settings remain the
authoritative administrative source.

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
`dd-build-server`.

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

## Fail-closed boundary

The independent lane rejects dynamic matrices, reusable workflows, conditions,
secret/OIDC expressions, arbitrary marketplace actions, service/job containers,
macOS/Windows execution, environments, deployments, caller-selected commands,
and mutable branch/tag execution.

A job can be supported by ARC while rejected by the independent mirror. This is
a normal and preserved distinction, not a failure to report.

## Existing build server integration

`dd-build-server` remains the executor and owns its existing containerd,
BuildKit, ECR, artifact, NATS, Postgres, and Fiducia boundaries. DEN-1550 adds
fixed `rust-verify`, `node-verify`, and `python-verify` profiles beside the
existing Flutter and browser profiles.

`gha-clone-server-rs` has no Kubernetes service-account token, hostPath, Docker
socket, containerd socket, BuildKit socket, cloud credentials, or production
kubeconfig.

## AWS and Hetzner

The GitOps resources are part of the shared `dd-next-runtime` render and
therefore can exist in AWS and Hetzner. The deployment begins at zero replicas.

- AWS is the initial independent execution authority because `dd-build-server`,
  BuildKit/containerd, ECR and its persistence already exist there.
- Hetzner can host non-privileged ARC scale sets immediately.
- A second independent executor in Hetzner requires shared artifact storage and
  Fiducia-fenced authoritative claims before activation.

## Secret contract

Backing secret: `dd/remote-dev/gha-clone-server-secrets`

Required properties:

- `auth_secret`
- `github_webhook_secret`
- `github_app_installation_token`
- `build_server_auth`

`github_app_installation_token` must be produced by the approved GitHub App
broker and rotated before expiry. Do not store a classic PAT in the backing
secret.

## Activation

1. Merge source, profiles, tests, workflow and GitOps resources.
2. Provision and validate the ExternalSecret without displaying values.
3. Build and digest-pin the ARC runner image.
4. Register `sonus-ci` with `minRunners: 0` and a bounded maximum.
5. Run the opt-in `arc-parity-smoke` workflow.
6. Scale `dd-gha-clone-server` from zero to one with execution still disabled.
7. Submit plan-only fixtures and compare classifications to the original GHA
   workflows.
8. Enable API execution only for trusted immutable commits.
9. Enable webhook execution only after HMAC, workflow fetch and idempotency
   evidence.
10. Keep positive GitHub-hosted budget for native platforms and emergency use.

## Rollback

- Set ARC scale-set maximum to zero or remove routing labels.
- Set `dd-gha-clone-server` replicas to zero and disable webhook rules.
- Do not bypass required checks. Restore hosted capacity or execute a reviewed
  equivalent with retained evidence.

## Repository boundary

No standalone `ORESoftware/gha-clone-server.rs` repository currently exists.
The first implementation remains in `ORESoftware/k8s-cluster` so source,
executor profiles, manifests and policy contracts are reviewed atomically.
Extraction is tracked through the protected repository-bootstrap workflow after
the API and golden fixtures stabilize.
