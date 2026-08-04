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
