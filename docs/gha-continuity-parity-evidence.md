# GitHub Actions continuity parity evidence

This repository uses four complementary CI lanes rather than claiming that one
custom service reimplements GitHub's proprietary Actions control plane:

1. **Hosted** — GitHub-hosted Linux runners while the organization has capacity.
2. **ARC on AWS** — the official Actions Runner Controller and GitHub runner
   protocol in the AWS cluster.
3. **ARC on Hetzner** — the same native protocol in the Hetzner cluster.
4. **Independent** — `gha-clone-server-rs` compiles a deliberately bounded,
   static workflow subset to fixed `dd-build-server` profiles.

ARC is the native-parity path. The independent lane is a continuity compiler,
not an arbitrary GitHub Actions interpreter. It rejects mutable revisions,
caller-selected commands, dynamic matrices, unreviewed marketplace actions,
secret expressions, service/job containers, OIDC, deployments, environments,
and approvals.

## Evidence contract

`scripts/ci/gha_continuity_evidence.py` records only stable, non-secret facts:

- exact `owner/repository`;
- exact lowercase 40-hex commit;
- workflow path below `.github/workflows`;
- deterministic `sha256:` planner identity;
- terminal success/failure status;
- named SHA-256 artifact digests;
- one fixed lane identifier.

The schema rejects unknown keys and keys that imply tokens, secrets,
credentials, commands, runner labels, environment values, or logs. Evidence is
therefore safe to retain as a GitHub Actions artifact and cannot become a second
execution API.

The comparator fails closed on:

- missing required lanes;
- duplicate lane evidence;
- different repositories, revisions, workflow paths, or plan IDs;
- different terminal outcomes;
- different artifact names or digests.

A successful comparison proves deterministic planner/artifact agreement for the
same immutable input. It does **not** prove macOS, Windows, mobile/KVM,
service-container, marketplace-action, approval, deployment, or secret parity.
Those remain explicit lane-specific capabilities.

## Automated gates

`.github/workflows/gha-continuity-parity.yml` runs three credential-free gates
on pull requests:

- Python schema/adversarial tests and actionlint;
- a real Chromium/APIRequest test against the compiled Rust server;
- hosted versus independent normalized planner evidence.

The browser test proves the real process remains fail closed:

- public descriptor, health, readiness, and capability routes work;
- planning requires `x-server-auth` and the repository allowlist;
- mutable revisions may be reviewed but cannot execute independently;
- independent execution remains disabled by default;
- unknown runs and unsupported methods do not reflect credentials.

## AWS and Hetzner ARC comparison

The two ARC evidence jobs are manual until both runner scale sets are registered
and healthy. Trigger **GHA continuity parity evidence** with
`run_arc_lanes=true`. The jobs target the `sonus-aws` and `sonus-hetzner`
runner groups with the `sonus-ci` scale-set label, verify the runner is non-root
and has no Docker/containerd/Kubernetes credential socket, and generate the same
normalized plan evidence.

The final job requires all four lanes when the manual flag is enabled. If either
scale set has no available runner, the workflow remains incomplete rather than
silently dropping that lane.

## Billing and activation

The current hosted pool is accepting jobs, so the fleet is not globally out of
GitHub-hosted capacity. Exact per-organization Actions usage and remaining
budget must come from the read-only billing GitHub App used by
`gha-capacity-broker-rs`; do not infer it from queue time or use a pasted PAT.

Activation remains staged:

1. reconcile GitHub App ExternalSecrets;
2. register AWS and Hetzner ARC scale sets;
3. run credential-free registration smokes;
4. run four-lane parity evidence;
5. enable organization routing variables through the capacity broker;
6. keep the independent execution flag disabled until fixed-profile admission,
   build-server authentication, NetworkPolicy, and live rollback evidence are
   all green.
