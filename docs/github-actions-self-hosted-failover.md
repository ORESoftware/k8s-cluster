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

# GitHub Actions capacity and self-hosted failover

Linear: `DEN-1549`; related: `DEN-378`, `DEN-381`, and `DEN-27`.

## Decision

Use GitHub's official Actions Runner Controller (ARC) for GitHub Actions execution compatibility on trusted Linux jobs. The first production target is the `sonus-auris` organization because a representative hosted job failed before any workflow step was created. Deploy a scale set named `sonus-ci` in both AWS and Hetzner and place the two scale sets in distinct organization runner groups, `sonus-aws` and `sonus-hetzner`.

`gha-clone-server-rs` lives in this repository but is a capacity and routing broker, not a clone of GitHub's proprietary workflow service. It reads current-month organization billing usage, evaluates an explicit policy, and can reconcile two selected-repository organization variables. The existing `dd-build-server` remains the bounded non-GitHub fallback for workflows that explicitly submit an operator-reviewed run profile.

The `ORESoftware` namespace is a personal GitHub account, not an organization. Organization runner groups and organization billing endpoints therefore cannot be attached to `ORESoftware/k8s-cluster`. Its own future runner lane must be repository-scoped or moved under an organization; this rollout does not falsely register an organization-level runner against that personal namespace.

## Current evidence and limits

A representative Sonus Auris run in `sonus-auris/sonus-auris-monorepo` (run `30566670766`, job `90952795806`) completed with failure before any step was created and had no downloadable job log. This is runner-allocation evidence rather than a failing product test. `DEN-378` records a 2,000-minute allowance exhausted on July 27, 2026.

The connected GitHub repository API does not expose the current numeric billing total. Do not infer a current August value from the July incident: included minutes reset by billing period. The broker queries GitHub's enhanced billing usage endpoint for the current UTC year and month. If that endpoint is unavailable or the App lacks access, policy fails closed rather than guessing.

## Execution lanes

| Lane | Workflow target | Purpose | Deliberate exclusions |
| --- | --- | --- | --- |
| GitHub-hosted Linux | `ubuntu-latest` | bootstrap, parity, special fallback | unavailable when hosted allocation is blocked by budget or policy |
| ARC Linux HA | `sonus-ci` | trusted Rust, Node, Python, Dart/Flutter analysis/test and repository-contract jobs | fork PRs, host sockets, service containers, production credentials, KVM, signing |
| Existing build server | `POST /builds` with `jobKind=run-profile` | allowlisted builds and CI/CD profiles that need cluster-local builders, artifacts, NATS, Postgres, or deploy controls | arbitrary workflow YAML, arbitrary shell commands, unreviewed repositories/profiles |
| Hosted/specialized | macOS, Windows, Android/KVM labels | platform-specific builds, emulators, signing, notarization | not replaced by the Linux ARC lane |

Self-hosted runner execution does not consume GitHub-hosted runner minutes, but GitHub remains the workflow control plane. If GitHub itself is unavailable, only jobs already designed to submit a bounded `dd-build-server` profile can continue.

## AWS/Hetzner active-active contract

Both clusters use ARC chart `0.14.2` and runner `2.334.0`.

- Both scale sets use `runnerScaleSetName: sonus-ci`.
- AWS uses runner group `sonus-aws`; Hetzner uses `sonus-hetzner`.
- GitHub assigns jobs by acquisition race; either cloud continues when the other is unavailable.
- GitHub assigns jobs by acquisition race; if one cluster is down, the other scale set continues acquiring jobs.
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
# GitHub Actions capacity and AWS/Hetzner failover

Linear: `DEN-1549`; continuity executor: `DEN-1550`; related security rotation: `DEN-27`.

## Decision

Keep GitHub as the normal workflow control plane and use GitHub's official Actions Runner Controller (ARC) for Linux job compatibility. The AWS and Hetzner clusters expose the same scale-set name, `sonus-ci`, in different organization runner groups:

- AWS: runner group `sonus-aws`;
- Hetzner: runner group `sonus-hetzner`.

A workflow that targets `sonus-ci` can therefore be acquired by either provider. If one cluster is unavailable or intentionally scaled down, the other remains eligible. The shared controller chart and both scale-set charts are pinned to ARC `0.14.2`; the general runner image is pinned to runner `2.334.0`.

`gha-clone-server-rs` remains the independent, bounded continuity executor merged under `DEN-1550`. It translates a deliberately small workflow subset into fixed `dd-build-server` profiles. It is not replaced by this work and it does not become an arbitrary shell executor.

The `gha-capacity-audit` binary is the capacity control-plane companion. It reads the current UTC month's organization billing usage and Actions budgets, classifies risk, emits JSON, and can optionally reconcile two non-secret organization variables for selected repositories:

- `CI_EXECUTION_MODE`;
- `CI_LINUX_RUNS_ON_JSON`.

Mutation is off by default. Unknown billing state never mutates routing.

## Why the billing result is gated

The connected repository API can show workflow allocation failures but cannot read organization billing totals. A representative Sonus Auris job failed before any step existed and had no log blob, which is consistent with runner allocation failure but is not a numeric quota report.

The audit uses these organization endpoints with a GitHub App installation token:

- `GET /organizations/{org}/settings/billing/usage` for current-month Actions minutes and amounts;
- `GET /organizations/{org}/settings/billing/budgets` for blocking or alerting budgets;
- optionally `PATCH`/`POST /orgs/{org}/actions/variables` for selected-repository routing.

The App must have organization **Administration: read**. Optional variable reconciliation additionally requires organization **Variables: write**. Do not use a classic PAT in Kubernetes, Git, workflow YAML, or logs.

The enhanced usage endpoint does not expose a plan's included-minute allowance directly, so `GHA_CAPACITY_INCLUDED_MINUTES` is explicit. Sonus Auris is currently configured as `2000`; do not copy that value to another organization without verifying its plan.

## Capacity states

| State | Evidence | Routing recommendation | Audit exit |
| --- | --- | --- | --- |
| `healthy` | Below 75% of the configured allowance and below budget thresholds | GitHub-hosted | 0 |
| `watch` | At least 75%, a positive net Actions charge, or at least 75% of an Actions budget | Keep hosted checks available; prepare ARC | 0 |
| `critical` | At least 90% of allowance/budget or included allowance reached | `sonus-ci` | 1 |
| `blocked` | A blocking Actions budget is zero or exhausted | `sonus-ci` | 1 |
| `unknown` | No usable allowance/budget evidence or API failure | Hold automatic mutation | 2 |

The audit CronJob is committed with `suspend: true` and `GHA_CAPACITY_MUTATION_ENABLED=false`. It runs only from an immutable `gha-clone-server` image containing the capacity binary; it never clones mutable source while holding the billing token. Unsuspending the audit is separate from enabling mutations.

## Workflow adoption contract

Only selected, trusted repositories may consume the routing variables. A compatible Linux job uses:

```yaml
runs-on: ${{ fromJSON(vars.CI_LINUX_RUNS_ON_JSON || '["ubuntu-latest"]') }}
```

The capacity broker writes either `["ubuntu-latest"]` or `["self-hosted","linux","sonus-ci"]`. Repository IDs are explicit; broad `all` or `private` organization-variable visibility is not supported by the broker.

Do not use this general lane for:

- public-fork pull requests or other untrusted code;
- macOS, iOS signing, Windows, or Android emulator/KVM jobs;
- workflows that require Docker/containerd host sockets, privileged pods, service containers, or Kubernetes credentials;
- production deployment secrets.

Those workloads remain on funded GitHub-hosted or separately isolated capability-specific lanes.

## Security boundary

Runner pods are one-job ephemeral pods with:

- non-root execution and RuntimeDefault seccomp;
- no service-account token;
- no `hostPath`, Docker socket, containerd socket, or privileged mode;
- bounded CPU, memory, ephemeral storage, pod count, and concurrency;
- no ingress;
- DNS plus public HTTPS egress while metadata and private address ranges are blocked.

ARC registration uses `sonus-auris-arc-github`, projected by External Secrets from `dd/ci/github-apps/sonus-auris-arc`. The capacity audit reuses the short-lived installation-token projection already owned by `dd-gha-clone-server`; its App permissions must be reviewed before activation.

## Activation sequence

1. Merge the manifests, audit binary, contract tests, and docs into `dev`.
2. Create Sonus Auris organization runner groups `sonus-aws` and `sonus-hetzner`; restrict both to the initial trusted repositories.
3. Create/install the ARC GitHub App and store `github_app_id`, `github_app_installation_id`, and `github_app_private_key` under `dd/ci/github-apps/sonus-auris-arc`.
4. Reconcile prerequisites in AWS and Hetzner and verify `sonus-auris-arc-github` is Ready without printing values.
5. Manually sync `dd-sonus-ci-runner-set-aws`; dispatch the credential-free smoke workflow and prove non-root/no-socket/no-Kubernetes-token isolation.
6. Manually sync `dd-sonus-ci-runner-set-hetzner`; stop or scale down AWS and prove that Hetzner acquires the same `sonus-ci` smoke job.
7. Grant the capacity App Administration read, run the audit manually, and record current minutes/budget JSON with credentials redacted.
8. Unsuspend `dd-gha-capacity-audit` while mutation remains false; observe at least one successful hourly report.
9. Adopt the routing expression in selected repository workflows and compare GitHub-hosted versus ARC outputs.
10. Grant Variables write and enable mutation only after parity, runner-group restrictions, and rollback have been demonstrated.

## Rollback

1. Set `GHA_CAPACITY_MUTATION_ENABLED=false` or suspend `dd-gha-capacity-audit`.
2. Set `CI_LINUX_RUNS_ON_JSON` to `["ubuntu-latest"]` for selected repositories.
3. Pause the provider scale-set Applications and allow ephemeral jobs to drain.
4. Leave the shared ARC controller in place until all runner pods and listeners have terminated.
5. Rotate the GitHub App key/installation token if compromise is suspected.
6. Do not rewrite workflow history, delete artifacts, or force-update deployment branches during rollback.

## Evidence required before declaring failover complete

- numeric current-month Actions usage from the authorized organization endpoint;
- both runner groups and repository allowlists;
- ExternalSecret readiness in both providers;
- AWS smoke, Hetzner smoke, and provider-loss acquisition proof;
- hosted-versus-ARC test parity;
- selected-only variable mutation audit;
- rollback drill;
- revocation of any classic PAT pasted into chat or another untrusted channel.

The controller and scale-set Argo CD Applications are intentionally manual until the operator confirms no older ARC controller/CRD installation conflicts with the `0.14.2` deployment. The prerequisites Application may reconcile the namespace, quotas, NetworkPolicy, and ExternalSecrets safely.

## Apps and secrets

Use separate least-privilege GitHub Apps per organization and per responsibility.

1. **ARC registration App**: organization self-hosted runners permission, installed on `sonus-auris`, stored at `dd/ci/github-apps/sonus-auris-arc` in AWS Secrets Manager, materialized as `sonus-auris-arc-github`.
2. **Capacity broker App**: organization Administration read and Variables write, installed on `sonus-auris` and limited to selected repositories, stored at `dd/ci/github-apps/sonus-auris-capacity-broker`, materialized as `sonus-auris-gha-capacity-broker`.

One broker deployment represents one organization and one GitHub App installation. Deploy another instance and secret for another organization; never reuse an installation ID as if it were cross-organization authority.

Do not use the classic PAT pasted into chat. Revoke and rotate it under `DEN-27`; never place it in Git, Kubernetes YAML, Actions variables, or logs.

## Capacity policy

The broker reads `GET /organizations/{org}/settings/billing/usage` with the current UTC `year` and `month`, then filters `product=Actions` and `unitType=minutes`.

- 75%: warning threshold.
- 90%: route opted-in trusted Linux jobs to certified ARC capacity.
- 100%: do not assume GitHub-hosted allocation succeeds.
- Billing API unavailable: use certified ARC capacity; otherwise hold.

The broker writes only `CI_EXECUTION_MODE` and `CI_LINUX_RUNS_ON_JSON`. Both variables use `visibility: selected` with an explicit repository-ID allowlist. Mutation defaults to false. A compatible trusted Linux job opts in with:

```yaml
runs-on: ${{ fromJSON(vars.CI_LINUX_RUNS_ON_JSON || '["ubuntu-latest"]') }}
```

`build-server` mode is a routing signal, not arbitrary command execution. A workflow may react to that mode only by calling an already reviewed `dd-build-server` profile. The build server independently enforces repository, profile, image, namespace, auth, and deployment allowlists.

## Activation sequence

1. Merge code, manifests, and credential-free contract tests to `dev`.
2. Confirm no active legacy ARC controller or incompatible `actions.github.com` CRDs exist in each target cluster. Follow the upstream clean-install/upgrade procedure when required.
3. Create Sonus runner groups `sonus-aws` and `sonus-hetzner`, restricted to trusted private repositories and approved workflows.
4. Create/install the ARC App and capacity-broker App; populate the two Secrets Manager records.
5. Reconcile prerequisites and confirm both ExternalSecrets are Ready without printing values.
6. Manually sync the AWS controller and scale set, then run the Sonus manual smoke workflow.
7. Manually sync Hetzner with the same scale-set name and its distinct group. Stop AWS acquisition temporarily and prove Hetzner failover.
8. Build and publish `gha-clone-server-rs`; scan it, produce SBOM/provenance, record an immutable digest, and promote the digest-gated deployment template.
9. Keep broker mutation disabled while comparing hosted and ARC output for representative Rust, Node, Flutter analysis, and browser jobs.
10. Set `selfHostedReady=true`, enable selected-repository variable mutation, and verify the 75/90/100 routing behavior.
11. Migrate required checks only after hosted-vs-ARC parity. Retain funded hosted capacity for special platforms and emergencies.

## Rollback

- Set `GHA_MUTATION_ENABLED=false`.
- Restore `CI_LINUX_RUNS_ON_JSON` to `["ubuntu-latest"]` for selected repositories.
- Pause both runner-scale-set Applications and allow ephemeral runners to terminate.
- Leave or remove the controller according to the upstream ARC clean-uninstall procedure; do not strand CRDs.
- Rotate the affected App key/installation after suspected compromise.
- Preserve build artifacts and workflow history.

## Completion evidence

- current-month numeric Actions usage from the authorized billing endpoint;
- runner-group creation and repository/workflow allowlists;
- ExternalSecret reconciliation in AWS and Hetzner;
- one AWS smoke, one Hetzner smoke, and a failover proof;
- hosted-vs-ARC parity results;
- immutable broker image digest, SBOM, provenance, and healthy deployment;
- variable mutation audit and rollback drill;
- evidence that no public-fork job can reach the self-hosted lane.
