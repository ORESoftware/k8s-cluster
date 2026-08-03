# GitHub Actions capacity, self-hosted execution, and CI continuity

Linear: `DEN-1549`; related: `DEN-1550`, `DEN-378`, `DEN-381`, `DEN-27`, and `DEN-1005`.

## Decision

Use four cooperating components rather than attempting to copy GitHub's proprietary control plane wholesale:

1. **GitHub-hosted runners** for bootstrap, comparison, and specialized hosted jobs while allocation is available.
2. **Official Actions Runner Controller (ARC)** for normal trusted Linux GitHub Actions compatibility on ephemeral self-hosted runners in both AWS and Hetzner.
3. **`gha-clone-server-rs`** as an independent, fail-closed workflow planner and fixed-profile dispatcher for continuity when a hosted run fails to allocate or GitHub-hosted capacity is intentionally avoided.
4. **`dd-build-server`** as the existing cluster-local build and CI/CD execution service for allowlisted, operator-reviewed run profiles, artifacts, container builds, and deployments.

`gha-capacity-broker-rs` selects among hosted, ARC, build-server, and hold modes. It is a capacity and routing broker, **not a clone of GitHub's proprietary workflow service**, and never parses or executes repository-supplied shell commands.

The first organization rollout is `sonus-auris`. Deploy a scale set named `sonus-ci` in both AWS and Hetzner, with distinct runner groups `sonus-aws` and `sonus-hetzner`. Reuse requires separate Apps, policies, runner groups, repository allowlists, parity evidence, and rollback ownership for each organization.

The `ORESoftware` namespace is a personal GitHub account, not an organization. Organization runner groups, organization billing, and organization variables cannot be attached to `ORESoftware/k8s-cluster`; its own lane must be repository-scoped or the repository must move under an organization.

## Current Actions status — August 3, 2026

GitHub-hosted Actions are **not globally exhausted** for the accessible repositories. On August 3, 2026, `ORESoftware/k8s-cluster` hosted jobs received Ubuntu runners and completed real steps with logs. Draft-branch `action_required` outcomes that created no jobs are approval/workflow state, not proof of a global minute outage.

The connected repository API does not expose the current numeric Sonus Auris balance. Do not infer August usage from the July DEN-378 incident because included usage resets by billing period.

The broker reads the current UTC month from the public-preview organization summary endpoint:

```text
GET /organizations/{org}/settings/billing/usage/summary?year=YYYY&month=M&product=Actions
```

The endpoint-specific GitHub REST contract supports GitHub App installation tokens with organization `Administration: read`. Because the endpoint is public preview, request/response drift is treated as a billing-read failure and routing fails closed.

## Billing quantity semantics

The summary response uses:

- `grossQuantity`
- `discountQuantity`
- `netQuantity`

For Actions minute capacity:

- **gross minutes** are the total minutes consumed before included-usage or other quantity discounts and are compared with the configured included-minute allowance;
- **net minutes** are the billable minutes after quantity discounts and are retained as cost telemetry only.

Using `netQuantity` as the capacity numerator could report zero while the included-minute allowance is fully consumed. The checked-in parser and tests therefore route on `grossQuantity`, record `netQuantity` separately, and reject a missing or malformed `usageItems` contract.

## Execution lanes

| Lane | Target | Purpose | Deliberate exclusions |
| --- | --- | --- | --- |
| GitHub-hosted Linux | `ubuntu-latest` | bootstrap, parity comparison, temporary fallback | unavailable when allocation or budget policy blocks hosted jobs |
| ARC Linux active-active | `sonus-ci` | trusted Rust, Node, Python, Dart/Flutter analysis/test and repository-contract jobs | fork PRs, host sockets, privileged builds, KVM, signing, production credentials |
| Independent continuity server | `gha-clone-server-rs` | webhook-driven planning, failure-only replay, fixed workflow-to-profile mapping, dispatch audit | arbitrary workflow YAML parity, arbitrary action execution, unreviewed commands |
| Existing build server | `POST /builds` with `jobKind=run-profile` | allowlisted builds, artifacts, image builds, cluster-local test services, controlled CI/CD/deploy profiles | arbitrary shell, arbitrary repository/profile/image/namespace, unreviewed deployments |
| Hosted/specialized | macOS, Windows, Android/KVM labels | platform builds, emulators, signing and notarization | not replaced by the Linux ARC lane |

Self-hosted ARC execution does not consume GitHub-hosted runner minutes, but GitHub remains the workflow control plane. If GitHub itself is unavailable, only fixed profiles already understood by `gha-clone-server-rs` and `dd-build-server` can continue.

## Compatibility and parity boundary

The target is practical parity, not a misleading claim of complete reimplementation:

- **Workflow/action compatibility:** official ARC runners execute the same checked-in trusted Linux jobs and actions as GitHub-hosted Linux, subject to runner image and security policy.
- **Control-plane continuity:** `gha-clone-server-rs` mirrors only reviewed workflows and fixed execution profiles. Unsupported expressions, dynamic actions, arbitrary commands, or unapproved repositories fail closed.
- **Build/deploy continuity:** `dd-build-server` provides pre-existing artifacts, builders, cluster-local dependencies, NATS/Postgres integration, and bounded deployment profiles.
- **Specialized platforms:** macOS, Windows, signing, notarization, and Android/KVM remain separate lanes.

A workflow is not parity-ready until hosted and ARC runs produce equivalent tests, artifacts, cache semantics, timeouts, cancellation behavior, and required-check conclusions.

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

## Three-App authority separation

Use three independent least-privilege GitHub Apps per organization.

1. **ARC registration App** — organization self-hosted-runners permission; secret source `dd/ci/github-apps/sonus-auris-arc`; projected as `sonus-auris-arc-github` and consumed only by ARC.
2. **Billing-read App** — organization `Administration: read`; secret source `dd/ci/github-apps/sonus-auris-billing`; projected as `sonus-auris-gha-billing` and used only for the current-month summary endpoint.
3. **Capacity-mutation App** — organization Variables write and only the minimum supporting read permissions; secret source `dd/ci/github-apps/sonus-auris-capacity-broker`; projected as `sonus-auris-gha-capacity-broker` and used only for selected-repository variables.

The broker mints separate short-lived installation tokens for Apps 2 and 3. It fails startup if they share an App installation or private-key path. App IDs, installation IDs, private keys, token caches, runtime mounts, logs, metrics, and rotation schedules remain separate.

The long-lived PAT pasted into chat is not used by this design. Revoke and rotate it under DEN-27.

## Capacity policy

The broker filters summary items where `product=Actions` and `unitType=minutes`, sums nonnegative finite `grossQuantity`, and compares the total with the configured included-minute allowance.

- 75%: warn.
- 90%: route opted-in trusted Linux jobs to certified ARC capacity.
- 100%: do not assume GitHub-hosted allocation succeeds.
- Billing API, App permission, or response unavailable: use certified ARC capacity; otherwise hold.

The broker writes only:

- `CI_EXECUTION_MODE`
- `CI_LINUX_RUNS_ON_JSON`

Both use `visibility: selected` with explicit positive, unique repository IDs. Hosted and self-hosted label sets must be nonempty, unique, whitespace-free, valid, and non-overlapping.

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
2. Confirm no temporary self-mutating workflow remains in the reviewed diff.
3. Audit active ARC controllers and CRDs in AWS and Hetzner.
4. Create runner groups `sonus-aws` and `sonus-hetzner`, restricted to trusted private repositories and approved workflows.
5. Create/install the ARC registration, billing-read, and capacity-mutation Apps.
6. Reconcile all three ExternalSecrets and verify file mounts without printing values.
7. Manually sync AWS controller and scale set and run the credential-free smoke workflow.
8. Manually sync Hetzner using the same scale-set name and its distinct group. Stop AWS acquisition temporarily and prove Hetzner failover.
9. Build `gha-capacity-broker-rs` and `gha-clone-server-rs`; scan images, generate SBOM/provenance, record immutable digests, and promote digest-gated deployments.
10. Keep mutation and independent execution disabled while comparing hosted, AWS ARC, Hetzner ARC, and fixed build-server profiles for representative Rust, Node, Flutter-analysis, and browser jobs.
11. Enable `selfHostedReady`, then selected-repository variable mutation, then failure-only continuity dispatch.
12. Migrate required checks only after parity, security isolation, cancellation, and rollback drills pass.

## Rollback

- Set `GHA_MUTATION_ENABLED=false` and independent workflow execution false.
- Restore selected repositories to hosted mode only when funded hosted capacity is confirmed; otherwise set hold.
- Pause both runner-scale-set Applications and allow ephemeral runners to terminate.
- Leave or remove the ARC controller according to the upstream clean-uninstall procedure; do not strand CRDs.
- Disable continuity webhooks and fixed-profile dispatch.
- Revoke/rotate the affected App key after suspected compromise.
- Preserve build artifacts, workflow history, decisions, and audit records.

## Completion evidence

- current-month numeric Actions usage from the authorized summary endpoint, including gross and net minute totals;
- proof that hosted runs are distinguished from approval/action-required failures;
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
