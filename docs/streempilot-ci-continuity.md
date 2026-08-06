# StreemPilot CI continuity rollout

Linear: DEN-1550

Related: DEN-1549, DEN-905, DEN-900, DEN-918

Last reviewed: 2026-08-04

## Current capacity evidence

Fresh August 3–4 GitHub-hosted jobs in `ORESoftware/k8s-cluster` and the
StreemPilot repositories received runners, executed steps, produced logs, and
completed. GitHub-hosted Actions are therefore not globally unavailable at the
moment this rollout was prepared.

That evidence does **not** reveal the exact remaining organization allowance.
The authoritative remaining-minute/cost decision is the organization billing
summary read by `gha-capacity-broker-rs` through its dedicated billing-read
GitHub App. Do not infer a balance from one successful run and do not reuse a
classic PAT for billing, ARC registration, mutation, or workflow execution.

## Architecture

StreemPilot uses the same four-lane continuity model as the rest of the fleet:

1. GitHub-hosted runners while funded capacity and platform requirements allow;
2. official Actions Runner Controller (ARC) scale sets in AWS and Hetzner for
   native trusted-Linux workflow compatibility;
3. `gha-clone-server-rs` for a fail-closed static subset compiled to fixed
   profiles;
4. `dd-build-server` for the reviewed profile execution, artifacts, and existing
   cluster CI/CD services.

The custom mirror is not advertised as a proprietary GitHub Actions clone. ARC
is the native-parity lane. The mirror exists so core verification can continue
when the GitHub runner allocation path or included-minute budget is unavailable.

## Exact repository coverage

The independent server allowlists only these StreemPilot repositories and one
workflow path in each:

| Repository | Mirror path | Fixed-profile coverage |
| --- | --- | --- |
| `StreemPilot/streempilot-api-server.rs` | `.github/workflows/ci-mirror.yml` | root Rust formatting, check, Clippy, tests |
| `StreemPilot/streempilot-web-server.rs` | `.github/workflows/ci-mirror.yml` | root Rust formatting, check, Clippy, tests |
| `StreemPilot/streempilot-interfaces` | `.github/workflows/ci-mirror.yml` | lockfile-strict Node contracts/TypeScript, then generated Rust bindings |

The API and web mirrors compile to `rust-verify`. Interfaces compiles to a
`node-verify` → `rust-verify` DAG. The Rust profile accepts a generated-interface
crate only when the reviewed repository shape is present:

- `generated/rust/Cargo.toml` and `Cargo.lock`;
- `schema/domain.schema.json`;
- `nats/subjects.json`.

There is no dynamic crate discovery.

## Deliberate exclusions

The independent StreemPilot mirror does not claim coverage for:

- Playwright/browser system dependencies or artifact upload;
- generated Dart analysis;
- service containers, Docker-in-Docker, Android/KVM, macOS, Windows, or signing;
- OIDC, environments, deployments, approvals, matrices, reusable workflows, or
  arbitrary marketplace actions;
- secret expressions, mutable branch/tag execution, caller-selected commands,
  images, labels, or working directories.

The original `ci.yml` workflows remain authoritative for full checks. Native
GitHub-hosted or ARC runners execute those semantics. The mirror workflows are
`workflow_dispatch` only, so checking them into the application repositories
does not add a second automatic hosted run or consume hosted minutes on every
pull request.

## Trigger and execution path

1. GitHub sends an allowlisted event to `/webhooks/github`, or an operator calls
   the authenticated `/v1/runs` endpoint.
2. The server verifies HMAC/operator auth, exact repository, exact workflow
   path, UUID delivery identity where applicable, and a 40-hex revision.
3. It fetches the mirror file at that immutable revision.
4. The planner validates static jobs and `needs`, returns ARC classification,
   and either produces fixed profiles or machine-readable rejection reasons.
5. The dispatcher submits only repository URL, immutable SHA, profile, and
   deterministic request ID to `dd-build-server`.
6. `dd-build-server` clones the same SHA and runs its operator-reviewed profile.
7. The independent run records every submission and terminal result.

Workflow-run fallback remains protected by the DEN-1550 delivery-hardening
train: terminal failure conclusion allowlisting, exact failed workflow path,
recursion exclusion, delivery UUID validation, and bounded replay suppression.

## Tests

The rollout includes:

- eight Rust planner tests against the exact three mirror fixtures;
- five TypeScript deployment/profile contracts;
- profile registry tests for the generated-interface fallback and optional
  TypeScript contract script;
- four API repository mirror tests;
- five web repository mirror tests;
- five interfaces repository mirror tests.

The tests prove exact profile mapping, deterministic DAG order, immutable-SHA
execution, manual-only triggers, approved setup actions, secret-free static
semantics, browser/Dart exclusion honesty, and no dynamic Cargo discovery.

## Activation sequence

1. Land the webhook hardening PR that this rollout is stacked on.
2. Land the three StreemPilot mirror workflow PRs.
3. Land this cluster allowlist/profile/fixture PR and promote it semantically to
   the Argo-tracked branch.
4. Provision separate short-lived GitHub App credentials for ARC registration,
   billing reads, and any routing mutation; reconcile External Secrets without
   displaying values.
5. Build immutable images with SBOM/provenance and replace source-at-startup
   deployment images before production enablement.
6. Register and smoke AWS and Hetzner ARC lanes. Record hosted-versus-ARC parity
   and provider-loss failover evidence.
7. Scale `dd-gha-clone-server` from zero to one with execution still disabled.
8. Submit all three fixtures through `/v1/plans` at merged commit SHAs and retain
   the plan evidence.
9. Enable API execution for exact reviewed repositories and run one build-server
   smoke per profile.
10. Register the HMAC webhook and enable webhook execution only after duplicate,
    recursion, and failed-delivery tests pass against the live endpoint.
11. Roll out capacity variables only to selected repositories and do not replace
    required checks until native ARC parity and rollback have been proven.

## Rollback

- Set mirror webhook and API execution flags to false and scale the deployment
  to zero.
- Set ARC scale-set maxima to zero or remove participating repository routing.
- Restore hosted routing where capacity exists.
- Never bypass required checks merely because one lane is unavailable.
- Preserve run, plan, build-server, and capacity-decision evidence for incident
  review.
