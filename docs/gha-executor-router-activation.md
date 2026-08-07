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
# GHA executor router activation controls

Linear: DEN-1597  
Parent: DEN-1550  
Core router: pull request #645  
Inert GitOps: pull request #650  
Last reviewed: 2026-08-04

## Boundary

The continuity system has two intentionally different lanes:

- Actions Runner Controller (ARC) keeps native GitHub Actions orchestration and
  workflow semantics.
- `gha-clone-server-rs` plus `gha-executor-router` provides a narrower
  independent lane for exact repositories, immutable commits, bounded workflow
  plans, and fixed `dd-build-server` profiles.

The independent lane is not a claim of full GitHub Actions parity. GitHub-owned
expressions, matrices, marketplace actions, checks, caches, artifacts,
environments, OIDC, macOS, and Windows remain on hosted runners or ARC.

## Inert merged state

The reviewed GitOps scaffold must remain inert after merge:

- clone server replicas: zero;
- executor router replicas: zero;
- clone-server API execution: false;
- clone-server webhook execution: false;
- executor-router execution: false;
- AWS: declared as the first reviewed fixed-profile authority;
- Hetzner: declared but disabled, with no URL and no authentication path.

No workflow dispatch, self-hosted runner, webhook, build, image publication, or
cloud deployment is activated by the GitOps merge.

## Network path

The only independent execution path is:

```text
dd-remote-gateway -> dd-gha-clone-server:8125
                    -> dd-gha-executor-router:8126
                    -> dd-build-server:8100
```

The clone server accepts ingress only from `dd-remote-gateway`. The former
`dd-build-server` ingress permission is removed because status polling and
submission are clone-server-initiated outbound calls through the router.

The clone server has no direct egress to `dd-build-server`. The router is the
only workload allowed to reach port 8100. The inert router has no generic
Internet egress; a later Hetzner activation must add one exact reviewed network
path together with its endpoint and credential.

Neither workload receives a Kubernetes service-account token, hostPath, Docker
socket, containerd socket, BuildKit socket, cloud credential, or production
kubeconfig.

## Credentials

The clone-to-router authority and router-to-AWS authority remain distinct.
Authentication is projected as direct-child mounted files under:

```text
/var/run/secrets/gha-executor-router
```

The route inventory contains file paths, never credential values. Disabled
executors must omit both endpoint and authentication-path state. Hetzner must
receive a third distinct authority in its activation pull request.

GitHub API reads remain a separate short-lived GitHub App installation token.
Classic personal access tokens are outside this design and must not be stored in
Git, manifests, workflow inputs, or backing router secrets.

## No-duplicate provider rule

Provider selection may occur only before `POST /builds`:

1. probe executors in reviewed order;
2. skip a provider only when readiness is already unavailable;
3. submit once to the first ready provider;
4. namespace the accepted build ID with that executor;
5. keep every status read pinned to the accepting executor.

After a submission attempt, a timeout, reset, redirect, 429, 5xx, oversized or
invalid acceptance, or response-body failure is ambiguous. The router must not
submit the same logical request to the other cloud. Operator reconciliation
uses the unchanged deterministic `requestId` and the build-server/Fiducia state.

Automatic post-attempt takeover remains blocked until there is shared durable
assignment, shared job and artifact visibility, and a Fiducia-fenced claim.

## Activation sequence

1. Merge the core router and inert GitOps stack in dependency order.
2. Replace in-pod source compilation with immutable digest-pinned clone-server
   and router images; retain SBOM, provenance, and vulnerability evidence.
3. Provision and rotate the distinct clone-to-router and router-to-AWS
   authorities without displaying values.
4. Verify ExternalSecret readiness and exact source/image identity in a
   non-production namespace.
5. Keep both Deployments at zero and render the complete AWS and Hetzner ArgoCD
   targets.
6. Scale only the router to one with execution false; verify health, readiness,
   capabilities, mounted-file constraints, metrics, and bounded logs.
7. Scale the clone server to one with API and webhook execution false; run
   plan-only and bounded meta-dogfood fixtures.
8. Enable router execution and submit one immutable AWS fixed-profile smoke;
   retain request ID, logs, status, and artifacts.
9. Enable clone-server API execution for one exact allowlisted workflow and
   prove the same AWS route.
10. Provision the Hetzner fixed-profile build server, immutable runner images,
    TLS endpoint, distinct credential, artifact addressing, egress policy, and
    rollback evidence.
11. Add the exact Hetzner URL, auth path, projected secret item, and narrow
    network path in one reviewed change.
12. Prove AWS readiness failure selects Hetzner before submission.
13. Prove AWS rejection and every ambiguous post-attempt outcome never touch
    Hetzner.
14. Enable webhook execution only after HMAC, exact-path, delivery-ID, replay,
    deduplication, retention, and recursion controls pass.
15. Add shared durable assignment and Fiducia fencing before permitting any
    post-attempt provider takeover.

## Rollback

Disable webhook execution, clone-server API execution, and router execution in
that order. Scale clone server and router to zero. Drain accepted builds before
removing or renaming an executor identity. Preserve request IDs, logs, artifacts,
and workflow evidence. Restore hosted or certified ARC capacity rather than
bypassing required checks.

## Hosted Actions allowance

A job that acquires a hosted Ubuntu runner proves runner allocation currently
works for that repository. It does not reveal the numeric included-minute
balance, spending limit, payment state, or organization policy. DEN-1549 remains
the billing-read and capacity-broker boundary; unavailable billing evidence must
select certified self-hosted capacity or hold, never a guessed number.
