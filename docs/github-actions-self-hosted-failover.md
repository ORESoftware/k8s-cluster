
# GitHub Actions capacity and self-hosted failover

Linear: `DEN-1549`; related: `DEN-378`, `DEN-381`, and `DEN-27`.

## Decision

Use GitHub's official Actions Runner Controller (ARC) for GitHub Actions execution compatibility on trusted Linux jobs. The first production target is the `sonus-auris` organization because a representative hosted job failed before any workflow step was created. Deploy a scale set named `sonus-ci` in both AWS and Hetzner and place the two scale sets in distinct organization runner groups, `sonus-aws` and `sonus-hetzner`.

`gha-capacity-broker-rs` lives in this repository but is a capacity and routing broker, not a clone of GitHub's proprietary workflow service. It reads current-month organization billing usage, evaluates an explicit policy, and can reconcile two selected-repository organization variables. The existing `dd-build-server` remains the bounded non-GitHub fallback for workflows that explicitly submit an operator-reviewed run profile.

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
- GitHub assigns jobs by acquisition race; if one cluster is down, the other scale set continues acquiring jobs.
- `minRunners` is zero and `maxRunners` is four per cloud.
- Runner pods are one-job ephemeral, non-root, socket-free, hostPath-free, and have no Kubernetes service-account token.
- CPU, memory, ephemeral storage, namespace quotas, and outbound network access are bounded.
- The general lane permits DNS and public HTTPS while blocking cloud metadata and private address ranges.

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
8. Build and publish `gha-capacity-broker-rs`; scan it, produce SBOM/provenance, record an immutable digest, and promote the digest-gated deployment template.
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

## Independent workflow mirror boundary

The quota and lane-selection service in this document is
`remote/deployments/gha-capacity-broker-rs`. It is intentionally separate from
`remote/deployments/gha-clone-server-rs`, which remains the fail-closed workflow
planner and fixed-profile dispatcher promoted through DEN-1550. The capacity
broker may select hosted, ARC, or reviewed build-server mode; it does not parse
or execute repository workflow commands and it never replaces the independent
mirror.
