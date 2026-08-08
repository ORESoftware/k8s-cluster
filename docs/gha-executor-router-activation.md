# GHA executor-router activation and rollback

Linear: DEN-1549, DEN-1550, DEN-1597

Status: **inactive review scaffold**

This runbook activates the bounded independent CI lane. It does not claim full
GitHub Actions parity. Native workflow parity remains GitHub Actions plus the
official Actions Runner Controller (ARC) scale sets on AWS and Hetzner.

Activation requires digest-pinned images, SBOM and provenance evidence, and a
separately reviewed change before either Deployment may leave `replicas: 0`.
Provider selection is pre-submit only; any post-attempt takeover requires one
Fiducia-fenced durable assignment.

## Merge-safe state

The committed GitOps state is intentionally incapable of starting:

- `dd-gha-clone-server` and `dd-gha-executor-router` use `replicas: 0`;
- clone API execution, clone webhook execution, and router execution are false;
- both Deployments reference an all-zero image digest sentinel;
- both containers have explicit binaries, read-only root filesystems, no service
  account token, no elevated capabilities, and no source-build shell;
- AWS is the only enabled independent executor;
- Hetzner has only a disabled provider identity, without a URL or credential;
- the router has no public egress; and
- the clone server cannot address `dd-build-server` directly.

Changing any of those facts requires a separately reviewed activation pull
request with the evidence below.

## Required immutable images

Build `gha-clone-server` and `gha-executor-router` from exact merged commits in a
reviewed builder. For each image, retain:

1. immutable registry digest;
2. source commit and reproducible build command;
3. SBOM;
4. provenance/attestation;
5. vulnerability scan and exception record;
6. runtime user and entrypoint evidence; and
7. signature verification evidence.

Replace only the all-zero digest sentinels. Never activate a mutable tag, branch
checkout, `cargo run`, or in-pod source compilation.

## Credential prerequisites

Use separate least-privilege authorities:

- clone API authentication;
- GitHub webhook HMAC;
- short-lived GitHub App installation token for allowlisted workflow reads;
- clone-to-router authentication;
- AWS router-to-build-server authentication; and
- a future distinct Hetzner router-to-build-server authentication value.

The AWS authority is the existing `dd-agent-secrets.SERVER_AUTH_SECRET`, projected
read-only into the router. Do not copy it into another backing secret. Do not use
a classic personal access token for ARC registration, webhook delivery, billing
reads, clone reads, or executor authentication.

Verify ExternalSecret readiness without printing secret values.

## Phase 1: inert image and readiness proof

1. Replace both all-zero digest sentinels with reviewed image digests.
2. Keep every execution flag false and both Deployments at `replicas: 0`.
3. Render the complete `dd-next-runtime` Kustomize overlay.
4. Confirm NetworkPolicies admit only the documented paths.
5. Scale the router to one while execution remains false.
6. Prove `/healthz`, `/readyz`, `/v1/capabilities`, and `/metrics` are
   source-redacted.
7. Scale the clone server to one while API and webhook execution remain false.
8. Prove plan-only behavior for one exact allowlisted repository and commit.

No build may be submitted in this phase.

## Phase 2: AWS fixed-profile smoke

1. Confirm the existing AWS `dd-build-server` is ready and its profile registry
   matches the tested commit.
2. Enable router execution for one replica only.
3. Keep webhook execution false.
4. Submit one exact immutable `rust-verify` fixture through the clone API.
5. Confirm the deterministic request ID reaches AWS unchanged.
6. Confirm the accepted route ID is namespaced to `aws-primary`.
7. Retry the identical request and verify it reattaches rather than creating a
   second build.
8. Retry the request ID with changed immutable inputs and verify conflict.
9. Verify logs, status, artifacts, and terminal result are provider-addressable.
10. Disable router execution after the smoke.

Restart-durable and cross-replica request identity is not yet claimed. Keep the
router and webhook execution at one replica until Postgres/Fiducia ownership is
implemented and tested.

## Phase 3: ARC native-parity proof

Provision the AWS and Hetzner ARC registration GitHub App, runner groups, and
ExternalSecrets. Keep zero warm runners and bounded maximums.

Run the opt-in `sonus-ci` smoke on each provider and compare the same trusted
Linux workload against GitHub-hosted Actions. Preserve check names, artifacts,
logs, exit status, cancellation, timeout, and ephemeral-runner behavior.

ARC remains the preferred self-hosted lane for workflows requiring native GitHub
Actions semantics.

## Phase 4: Hetzner independent executor

Do not edit the disabled Hetzner entry until all of these exist:

1. fixed-profile build server from an immutable digest-pinned image;
2. reviewed TLS identity or private-network route;
3. distinct mounted authentication value;
4. shared artifact retention and provenance policy;
5. provider-specific health, metrics, logs, and alerts;
6. exact egress policy for the endpoint, never generic Internet access;
7. provider-loss test environment; and
8. rollback proof.

Enable the URL and auth path in the same reviewed change that adds the exact
network route.

## Provider-loss and no-duplicate tests

The version-1 failover boundary is **pre-submit only**:

- if AWS `/readyz` is unavailable before submission, the router may select
  Hetzner;
- after any `POST /builds` attempt, transport failure, timeout, redirect, HTTP
  429, HTTP 5xx, oversized body, or malformed acceptance is ambiguous and must
  not fall through to another provider;
- HTTP 4xx contract rejection fails closed; and
- accepted status remains pinned to the accepting executor.

Prove both cases:

1. AWS unavailable before POST selects Hetzner and sends exactly one POST.
2. AWS accepts and then becomes unreachable; polling fails without any Hetzner
   POST.

Automatic takeover after POST requires one shared durable job/artifact record
and a Fiducia-fenced authoritative provider assignment. Do not approximate that
transaction with retries or readiness probes.

## Webhook activation

Webhook execution is last. Before enabling it, prove:

- raw-body HMAC verification;
- delivery UUID validation;
- exact repository/workflow allowlists;
- immutable `head_sha` compilation;
- failure-conclusion allowlist;
- recursion exclusion;
- duplicate-delivery behavior;
- bounded retention; and
- one active execution replica until a shared durable claim exists.

## Rollback

1. Set clone API, webhook, and router execution flags to false.
2. Scale clone server and router to `replicas: 0`.
3. Restore clone routing directly to the reviewed AWS build server only if the
   router itself is the failed component and the direct path has a separately
   approved credential and NetworkPolicy change.
4. Disable the Hetzner entry before removing its route or credential.
5. Set ARC scale-set maxima to zero or restore hosted runner labels.
6. Preserve required checks; never bypass them because a continuity lane is
   unavailable.
7. Retain incident logs, deterministic request IDs, provider assignments,
   artifacts, and image attestations.

The rollback is complete only when no continuity pod is running, no execution
flag is true, no dormant provider route remains, and hosted/native required
checks are restored.
