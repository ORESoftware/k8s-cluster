# Daedalus GitHub Actions continuity on AWS and Hetzner

## Scope

This runbook adds reviewable Linux CI continuity for the `daedalus-fab`
organization while preserving GitHub Actions semantics where they matter.

It is **not a reimplementation of GitHub Actions**:

- GitHub's official ARC controller and runner execute ordinary trusted Linux
  Actions jobs on AWS or Hetzner.
- `gha-clone-server-rs` is a bounded, fail-closed compiler for a deliberately
  smaller workflow subset.
- `dd-build-server` executes only reviewed fixed profiles and immutable Git
  commits; it does not accept workflow-provided commands, images, URLs, runner
  labels, or secret expressions.

The old architecture draft `ORESoftware/k8s-cluster#617` remains design
provenance. The current implementation stack is:

1. `#637` — current-`dev` webhook authenticity, workflow-path, failure-state,
   recursion, immutable-SHA, and delivery-deduplication hardening;
2. this PR — Daedalus organization ARC lanes on AWS and Hetzner plus a
   fail-closed cross-lane readiness contract;
3. `#639` — exact immutable-SHA checkout in `dd-build-server`, stacked through
   the Messaging Intel real-binary integration.

## Account and billing boundary

`daedalus-fab` is a GitHub organization, so organization runner groups,
organization Apps, and the existing `gha-capacity-broker-rs` model are valid.

`ORESoftware/k8s-cluster` is owned by a personal account. GitHub organization
billing and runner-group APIs do not apply to a personal account. Use a
repository-scoped runner registration or the fixed-profile build-server path for
that repository; never invent an organization billing response for it.

The connected GitHub surface can show whether hosted workflow runs start and
finish. It does not expose the exact remaining Actions billing balance. A green
hosted run proves only that hosted execution was available for that run, not how
many included minutes remain or when an organization will hit its limit.

## Topology

```text
GitHub hosted workflow failure
        |
        | signed workflow_run webhook, exact repository/path/SHA
        v
gha-clone-server-rs
        |
        +---- fixed reviewed profile ----> dd-build-server
        |
        +---- normal trusted Actions job remains on official ARC
                                      |
                        +-------------+-------------+
                        |                           |
                 daedalus-ci                  daedalus-ci
                 group daedalus-aws           group daedalus-hetzner
                 AWS cluster                  Hetzner cluster
```

Both ARC scale sets advertise the reviewed workflow label `daedalus-ci`.
Provider identity is never selected by webhook payloads or workflow inputs; it
is recorded by the immutable cloud manifest as `DD_CI_CLOUD=aws` or
`DD_CI_CLOUD=hetzner`. The status evaluator prefers Hetzner, then AWS, only when
the corresponding fixed runner group and scale-set identity are certified.

## Safe defaults

- The namespace and policy prerequisite Application may reconcile.
- ARC controller and scale-set Applications are manual and carry explicit
  activation annotations.
- `minRunners: 0` in both clouds avoids warm-capacity spend before certification.
- No host socket, privileged container, hostPath, or Kubernetes service-account
  token is available to runner jobs.
- Runner ingress is denied. Egress is limited to DNS and public TCP 443 while
  private, loopback, link-local, and carrier-grade NAT ranges are excluded.
- The GitHub App ExternalSecret and manual smoke are templates excluded from
  Kustomize.
- The checked-in continuity snapshot is all-false and exits fail closed.

## Activation sequence

### 1. Measure and record hosted execution

Record recent Actions run IDs, start/completion timestamps, and runner names for
the required Daedalus workflows. Check the organization billing page or the
organization billing API with an owner/billing-manager credential outside this
repository. Do not paste billing credentials into an issue, PR, or chat.

### 2. Audit ARC ownership

Before syncing another controller, inventory existing
`gha-runner-scale-set-controller` releases and ARC CRDs in the target cluster.
One reviewed release must own the CRD lifecycle. A second release must not
silently adopt or downgrade cluster-scoped resources.

### 3. Create organization authority

Create one dedicated `daedalus-fab` ARC GitHub App. Keep it distinct from:

- any billing-read App used by `gha-capacity-broker-rs`;
- any repository-variable mutation App;
- any human PAT or GitHub CLI session;
- the App used by another product organization.

Store the App ID, installation ID, and private key under
`dd/ci/github-apps/daedalus-fab-arc`. Review and intentionally materialize the
ExternalSecret template only after the installation covers the intended
repositories.

### 4. Create runner groups

Create organization runner groups:

- `daedalus-aws`
- `daedalus-hetzner`

Restrict each group to the reviewed Daedalus repositories. Do not grant all
public repositories or fork pull requests access to secret-bearing self-hosted
capacity.

### 5. Activate one cloud at a time

Sync the controller and scale set for one cloud. Keep `minRunners: 0`. Confirm:

```sh
kubectl -n arc-runners-daedalus get autoscalingrunnerset,autoscalinglisteners,ephemeralrunner
```

Copy the manual smoke template into a reviewed private Daedalus repository,
commit it, and run it with `workflow_dispatch`. Capture the exact commit, run
URL, provider output, runner name, and successful non-privileged assertions.

Only then mark that provider `configured`, `registered`, and `smokePassed` in a
sanitized operational snapshot. Repeat for the other cloud.

### 6. Evaluate continuity

Static snapshot:

```sh
python3 scripts/ops/gha_continuity_status.py \
  --snapshot /secure/path/daedalus-continuity.json \
  --require arc
```

Live bridge/build-server readiness plus stored ARC certification:

```sh
python3 scripts/ops/gha_continuity_status.py \
  --snapshot /secure/path/daedalus-continuity.json \
  --bridge-url http://dd-gha-clone-server.dd-next-runtime.svc.cluster.local:8125 \
  --build-server-url http://dd-build-server.dd-next-runtime.svc.cluster.local:8082 \
  --require either
```

Exit codes:

- `0`: at least the required reviewed lane is ready;
- `2`: valid evidence, but no required lane is ready;
- `3`: malformed, ambiguous, unreachable, or oversized evidence.

### 7. Prove hosted-vs-ARC parity

For every required workflow moved to `daedalus-ci`, compare:

- exact source commit and dependency locks;
- test counts and skipped tests;
- generated outputs and artifact hashes;
- Postgres/RLS or browser infrastructure differences;
- timeouts, cancellations, required-check names, and retention;
- absence of ambient cloud or cluster credentials.

Do not replace a required hosted check merely because one smoke job passed.

### 8. Enable failure continuity

After `#637` and the immutable build-server checkout fix land, configure exact
repository/workflow rules, reconcile the HMAC secret and GitHub fetch authority,
verify `/readyz`, then enable webhook execution. Start with one non-required
canary workflow and one exact repository.

## Capacity routing

For `daedalus-fab`, the existing capacity broker can route selected repositories
between GitHub-hosted and certified ARC capacity using organization billing
evidence and reviewed repository variables. Unknown billing must fail closed to
certified ARC or hold. Build-server mode applies only when the failed workflow
compiles to fixed profiles.

Provider preference is operational, not a security input. The checked-in status
evaluator uses `hetzner,aws` by default to reduce cost while retaining AWS as an
independent fallback. Operators may reverse that bounded order, but may not add
an arbitrary provider, group, or runner label.

## Rollback

1. Disable or remove the organization failure webhook.
2. Set webhook execution false on `gha-clone-server-rs`.
3. Set scale-set `maxRunners` to zero and allow active jobs to finish.
4. Remove the runner-set Application for the affected cloud.
5. Remove its controller only after proving no other scale set uses it.
6. Restore reviewed repository variables to hosted labels or hold.
7. Retain sanitized run, smoke, and readiness evidence for diagnosis.

## Credential response

A PAT pasted into chat or issue text is exposed. Revoke it immediately, review
its audit log, and rotate affected credentials. This architecture uses narrowly
scoped GitHub Apps and secret-manager projections; the exposed PAT is not an
activation dependency.
