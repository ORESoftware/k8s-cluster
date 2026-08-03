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
