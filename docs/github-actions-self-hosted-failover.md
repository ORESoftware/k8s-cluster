# GitHub Actions capacity, self-hosted execution, and CI continuity

Linear: `DEN-1549`; related: `DEN-1550`, `DEN-378`, `DEN-381`, `DEN-27`, and `DEN-1005`.

## Decision

Use four cooperating components rather than attempting to copy GitHub's proprietary control plane wholesale:

1. **GitHub-hosted runners** for bootstrap, comparison, and specialized hosted jobs while allocation is available.
2. **Official Actions Runner Controller (ARC)** for normal trusted Linux GitHub Actions compatibility on ephemeral self-hosted runners in both AWS and Hetzner.
3. **`gha-clone-server-rs`** as an independent, fail-closed workflow planner and fixed-profile dispatcher for continuity when a hosted run fails to allocate or GitHub-hosted capacity is intentionally avoided.
4. **`dd-build-server`** as the existing cluster-local build and CI/CD execution service for allowlisted, operator-reviewed run profiles, artifacts, container builds, and deployments.

`gha-capacity-broker-rs` selects among hosted, ARC, build-server, and hold modes. It is a capacity and routing broker, **not a clone of GitHub's proprietary workflow service**, and never parses or executes repository-supplied shell commands.

The first organization rollout is `sonus-auris`. Deploy a scale set named `sonus-ci` in both AWS and Hetzner, with distinct runner groups `sonus-aws` and `sonus-hetzner`. The same architecture is reusable per organization only after separate Apps, billing credentials, policies, runner groups, and repository allowlists are created.

The `ORESoftware` namespace is a personal GitHub account, not an organization. Organization runner groups and organization billing endpoints cannot be attached to `ORESoftware/k8s-cluster`; its own lane must be repository-scoped or the repository must move under an organization.

## Current Actions status — August 3, 2026

GitHub-hosted Actions are **not globally exhausted** for the accessible repositories. On August 3, 2026, `ORESoftware/k8s-cluster` secret-scan run `30847081509` completed successfully and repo-check run `30847081510` entered execution. Other draft-branch runs ended as `action_required` before creating jobs; that is workflow/approval state, not proof of a global minute outage.

The connected repository API does not expose the current numeric billing balance. Do not infer the August value from the July incident recorded in DEN-378, because included usage resets by billing period. The production broker must query the current UTC month and fail closed if billing data is unavailable.

GitHub's current organization billing summary route is:

```text
GET /organizations/{org}/settings/billing/usage/summary?year=YYYY&month=M
```

GitHub's billing documentation requires a billing-authorized classic PAT for this reporting API. A GitHub App installation token is not used for the billing read. The broker therefore separates billing authorization from organization-variable mutation.

## Execution lanes

| Lane | Target | Purpose | Deliberate exclusions |
| --- | --- | --- | --- |
| GitHub-hosted Linux | `ubuntu-latest` | bootstrap, parity comparison, temporary fallback | unavailable when allocation or budget policy blocks hosted jobs |
| ARC Linux active-active | `sonus-ci` | trusted Rust, Node, Python, Dart/Flutter analysis/test and repository-contract jobs | fork PRs, host sockets, privileged builds, KVM, signing, production credentials |
| Independent continuity server | `gha-clone-server-rs` | webhook-driven planning, failure-only replay, fixed workflow-to-profile mapping, dispatch audit | arbitrary workflow YAML parity, arbitrary action execution, unreviewed commands |
| Existing build server | `POST /builds` with `jobKind=run-profile` | allowlisted builds, artifacts, image builds, cluster-local test services, and controlled CI/CD/deploy profiles | arbitrary shell, arbitrary repository/profile/image/namespace, unreviewed deployments |
| Hosted/specialized | macOS, Windows, Android/KVM labels | platform builds, emulators, signing and notarization | not replaced by the Linux ARC lane |

Self-hosted ARC execution does not consume GitHub-hosted runner minutes, but GitHub remains the workflow control plane. If GitHub itself is unavailable, only fixed profiles already understood by `gha-clone-server-rs` and `dd-build-server` can continue.

## Compatibility and parity boundary

The target is practical parity, not a misleading claim of complete reimplementation:

- **Workflow/action compatibility:** official ARC runners execute the same checked-in trusted Linux jobs and actions as GitHub-hosted Linux, subject to the runner image and security policy.
- **Control-plane continuity:** `gha-clone-server-rs` mirrors only reviewed workflows and fixed execution profiles. Unsupported expressions, dynamic actions, arbitrary commands, or unapproved repositories fail closed.
- **Build/deploy continuity:** `dd-build-server` provides pre-existing artifacts, builders, cluster-local dependencies, NATS/Postgres integration, and bounded deployment profiles.
- **Specialized platforms:** macOS, Windows, signing, notarization, and Android/KVM remain separate lanes.

A workflow is not declared parity-ready until hosted and ARC runs produce equivalent tests, artifacts, cache semantics, timeouts, cancellation behavior, and required-check conclusions.

## AWS/Hetzner active-active ARC contract

Both clusters use ARC chart `0.14.2` and runner `2.334.0`.

- Both scale sets use `runnerScaleSetName: sonus-ci`.
- AWS uses runner group `sonus-aws`; Hetzner uses `sonus-hetzner`.
- GitHub assigns jobs by acquisition race; either cloud continues when the other is unavailable.
- `minRunners` is zero and `maxRunners` is four per cloud.
- Runner pods are one-job ephemeral, non-root, socket-free, hostPath-free, and have no Kubernetes service-account token.
- CPU, memory, ephemeral storage, namespace quotas, and outbound network access are bounded.
- The general lane permits DNS and public HTTPS while blocking cloud metadata and private address ranges.
- Public-fork and untrusted external-contribution workflows cannot use the self-hosted group.

Controller and scale-set Argo CD Applications remain manual until the operator confirms that no incompatible ARC controller or `actions.github.com` CRDs are installed. Prerequisite namespaces, quotas, NetworkPolicies, and ExternalSecrets may reconcile independently.

## Credential and authority separation

Use separate least-privilege credentials per organization and responsibility.

1. **ARC registration App** — organization self-hosted-runners permission; secret source `dd/ci/github-apps/sonus-auris-arc`; projected as `sonus-auris-arc-github`.
2. **Capacity mutation App** — organization Variables write and only the minimum administration/read permissions needed for selected-repository variable management; secret source `dd/ci/github-apps/sonus-auris-capacity-broker`; projected as `sonus-auris-gha-capacity-broker`.
3. **Billing read identity** — a dedicated organization owner or billing-manager identity with a classic PAT used only for billing summaries; secret source `dd/ci/github-billing/sonus-auris`; projected independently as `sonus-auris-gha-billing` and mounted only at `/var/run/gha-billing/token`.

The App private key and billing token must not share a file, Secret key, environment variable, workflow input, log, metric, response, or rotation schedule. The billing token is optional while the deployment remains inert; when absent, numeric billing is unavailable and policy fails closed.

Do not use the classic PAT pasted into chat. Revoke and rotate it under DEN-27. Future activation uses a newly created, dedicated billing-only credential through the approved secret store.

## Capacity policy

The broker filters billing summary items where `product=Actions` and `unitType=minutes`, then compares the total with the configured included-minute allowance.

- 75%: warn.
- 90%: route opted-in trusted Linux jobs to certified ARC capacity.
- 100%: do not assume GitHub-hosted allocation succeeds.
- Billing API or billing credential unavailable: use certified ARC capacity; otherwise hold.

The broker writes only:

- `CI_EXECUTION_MODE`
- `CI_LINUX_RUNS_ON_JSON`

Both use `visibility: selected` with explicit positive, unique repository IDs. Hosted and self-hosted label sets must be nonempty, unique, whitespace-free, and non-overlapping.

For `build-server` and `hold`, `CI_LINUX_RUNS_ON_JSON` is set to the deliberately nonexistent label `ci-capacity-hold-no-runner` rather than invalid empty JSON. Participating jobs must gate on execution mode:

```yaml
jobs:
  test:
    if: vars.CI_EXECUTION_MODE == 'hosted' || vars.CI_EXECUTION_MODE == 'self-hosted'
    runs-on: ${{ fromJSON(vars.CI_LINUX_RUNS_ON_JSON || '["ubuntu-latest"]') }}
    steps:
      - run: ./ci/test.sh
```

`build-server` is a routing signal, not arbitrary command execution. The continuity service may react only by dispatching an already reviewed build-server profile. Required checks are not migrated until skip/hold semantics and hosted-vs-ARC parity are proven.

## Activation sequence

1. Merge credential-free code, manifests, tests, and documentation to `dev`.
2. Remove temporary self-mutating merge-helper workflows from the reviewed branch.
3. Confirm no active legacy ARC controller or incompatible CRDs exist in AWS or Hetzner.
4. Create runner groups `sonus-aws` and `sonus-hetzner`, restricted to trusted private repositories and approved workflows.
5. Create/install the ARC App and capacity-mutation App.
6. Create a new billing-only classic PAT under a dedicated billing-manager identity; store it separately and record rotation/revocation ownership.
7. Reconcile prerequisites and prove all three ExternalSecrets are Ready without printing values.
8. Manually sync the AWS controller and scale set and run the credential-free smoke workflow.
9. Manually sync Hetzner using the same scale-set name and its distinct group. Stop AWS acquisition temporarily and prove Hetzner failover.
10. Build `gha-capacity-broker-rs` and `gha-clone-server-rs`; scan images, create SBOM/provenance, record immutable digests, and promote digest-gated deployments.
11. Keep mutation and independent execution disabled while comparing hosted, AWS ARC, Hetzner ARC, and fixed build-server profiles for representative Rust, Node, Flutter-analysis, and browser jobs.
12. Enable `selfHostedReady`, then selected-repository variable mutation, then failure-only continuity dispatch.
13. Migrate required checks only after parity, security isolation, cancellation, and rollback drills pass.

## Rollback

- Set `GHA_MUTATION_ENABLED=false` and independent workflow execution false.
- Restore selected repositories to hosted mode only when funded hosted capacity is confirmed; otherwise set hold.
- Pause both runner-scale-set Applications and allow ephemeral runners to terminate.
- Leave or remove the ARC controller according to the upstream clean-uninstall procedure; do not strand CRDs.
- Disable continuity webhooks and fixed-profile dispatch.
- Rotate the affected App key or billing credential after suspected compromise.
- Preserve build artifacts, workflow history, decisions, and audit records.

## Completion evidence

- current-month numeric Actions usage from the authorized `/usage/summary` endpoint;
- proof that recent hosted runs are distinguished from approval/action-required failures;
- runner-group creation and repository/workflow allowlists;
- three ExternalSecrets reconciled in AWS and Hetzner without value disclosure;
- one AWS smoke, one Hetzner smoke, and a cloud failover proof;
- hosted-vs-AWS-ARC-vs-Hetzner-ARC parity results;
- fixed-profile continuity and build-server E2E results;
- immutable image digests, SBOMs, provenance, and healthy deployments;
- variable-mutation and webhook-delivery audit;
- rollback drill;
- evidence that no public-fork job can reach self-hosted or build-server lanes.

## Independent services

`remote/deployments/gha-capacity-broker-rs` owns billing-aware lane selection and selected-repository variables.

`remote/deployments/gha-clone-server-rs` owns fail-closed workflow planning, webhook deduplication, reviewed workflow mappings, and fixed-profile dispatch.

`dd-build-server` owns allowlisted execution, builders, artifacts, and controlled CI/CD profiles.

These responsibilities remain separate. No service may silently absorb another service's credentials or arbitrary execution authority.
