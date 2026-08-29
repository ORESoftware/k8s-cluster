# Sonus Auris ARC and CI continuity plan

Status: **implementation-ready, activation gated**. The branch adds reviewable code and GitOps declarations but does not claim runner registration, parity, billing access, or required-check migration before operator gates are satisfied.

Linear: `DEN-381` (durable self-hosted runners), `DEN-1549` (capacity broker and AWS/Hetzner failover), `DEN-1550` (independent workflow mirror), `DEN-378` (hosted-minute incident), and `DEN-27` (credential rotation).

## Problem and current evidence

On July 27, 2026, the private `sonus-auris` organization recorded exhaustion of a 2,000-minute GitHub-hosted Actions allowance. A representative run failed before any workflow step existed and had no downloadable job log. That is runner-allocation evidence, not a product test result.

Included usage resets by billing period. On August 3, 2026, multiple `ORESoftware/k8s-cluster` jobs entered execution and completed successfully, proving hosted Actions are not globally exhausted. The connected repository surface still cannot establish the exact current Sonus numeric balance.

`gha-capacity-broker-rs` queries the current UTC year and month from the public-preview endpoint:

```text
GET /organizations/{org}/settings/billing/usage/summary?year=YYYY&month=M&product=Actions
```

The endpoint-specific GitHub contract supports installation access tokens with organization `Administration: read`. The broker uses a dedicated billing-read App, not the ARC registration App, variable-mutation App, or the PAT pasted into chat.

The response reports gross, discounted, and net quantities. Capacity thresholds use gross Actions minutes because included usage is represented as a discount; net minutes are retained separately for cost telemetry.

## Architecture decision

1. Use GitHub's official ARC controller and runner for normal trusted Linux workflow/action compatibility.
2. Deploy ARC independently in AWS and Hetzner after a controller/CRD audit.
3. Register two scale sets with `runnerScaleSetName: sonus-ci` and groups `sonus-aws` and `sonus-hetzner`.
4. Keep runners ephemeral, non-root, token-free, socket-free, hostPath-free, network-bounded, and resource-bounded.
5. Run one `gha-capacity-broker-rs` instance per organization. It reads billing through a dedicated read-only App and writes only two selected-repository variables through a distinct mutation App.
6. Preserve `gha-clone-server-rs` as the independent fail-closed workflow planner and fixed-profile dispatcher.
7. Preserve `dd-build-server` as the pre-existing bounded executor for approved run profiles, artifacts, image builds, NATS/Postgres-backed work, and controlled deploys.

`gha-capacity-broker-rs` is not a clone of GitHub's proprietary workflow service. ARC provides normal Linux runner compatibility; the independent mirror provides only reviewed workflow/profile continuity and rejects unsupported semantics.

The `ORESoftware` namespace is a personal account, not an organization. Organization runner groups, billing, and variables cannot be attached to `ORESoftware/k8s-cluster`; the first organization rollout targets `sonus-auris`.

## Capability-separated lanes

| Label/system | Initial purpose | Privilege posture |
| --- | --- | --- |
| `sonus-ci` | Rust, Node, Python, Dart, Flutter analysis/test/web/Linux, docs and repository contracts | non-root, no host sockets/mounts, no Kubernetes token, no KVM |
| `sonus-browser` | future browser-heavy checks | non-root browser image, bounded egress, no production credentials |
| `sonus-container` | future service-container or OCI work | separate rootless/isolated threat model; no host socket |
| `sonus-android` | future emulator/device integration | KVM/device-isolated nodes |
| `gha-clone-server-rs` + `dd-build-server` | fixed-profile continuity and build/deploy work | allowlisted repository/profile/image/namespace/auth; no arbitrary shell |

macOS/iOS and Windows remain hosted or use dedicated hardware fleets.

## Controller and chart lifecycle

Before syncing ARC `0.14.2`:

1. inventory active controllers, namespaces, service accounts, releases, and `actions.github.com` CRDs in both clusters;
2. decide whether an existing compatible controller can be generalized or a clean isolated controller is required;
3. follow upstream clean-install/upgrade guidance;
4. keep controller and scale-set chart versions identical;
5. stage AWS, then Hetzner.

The controller and scale-set Applications remain manual. Only prerequisites reconcile automatically.

## Three-App authentication

### ARC registration App

Install a dedicated App on `sonus-auris` with required self-hosted-runner permissions. Store App ID, installation ID, and private key at `dd/ci/github-apps/sonus-auris-arc`; project `sonus-auris-arc-github`.

### Billing-read App

Install a separate App with organization `Administration: read`. Store it at `dd/ci/github-apps/sonus-auris-billing`; project `sonus-auris-gha-billing`; mount its key only at `/var/run/gha-billing-app/github_app_private_key`.

### Capacity-mutation App

Install a third least-privilege App for the two selected-repository Actions variables. Store it at `dd/ci/github-apps/sonus-auris-capacity-broker`; project `sonus-auris-gha-capacity-broker`; mount its key only at `/var/run/gha-mutation-app/github_app_private_key`.

The broker mints and caches separate short-lived installation tokens. It rejects a shared App installation or shared private-key path at startup. App secrets are never present in runner pods.

Revoke the PAT pasted into chat under DEN-27; it is not used by this plan.

## Scale-set defaults

Both clouds use:

```yaml
githubConfigUrl: https://github.com/sonus-auris
githubConfigSecret: sonus-auris-arc-github
runnerScaleSetName: sonus-ci
minRunners: 0
maxRunners: 4
```

AWS uses `runnerGroup: sonus-aws`; Hetzner uses `runnerGroup: sonus-hetzner`. Job acquisition is active-active. Each pod uses non-root identity, dropped capabilities, RuntimeDefault seccomp, explicit limits, bounded emptyDir workspaces, no service-account token, and egress policy blocking metadata/private networks.

## Capacity policy

The broker filters current-month summary items where `product=Actions` and `unitType=minutes`.

- capacity numerator: nonnegative finite `grossQuantity`;
- cost telemetry: nonnegative finite `netQuantity`;
- 75% gross usage: warning;
- 90% gross usage: route opted-in trusted Linux jobs to certified ARC;
- 100% gross usage: do not assume hosted allocation succeeds;
- billing unavailable + ARC certified: use `sonus-ci`;
- billing unavailable + ARC unready: hold;
- hard stop + ARC unready + reviewed build-server path: report `build-server` for independent fixed-profile dispatch.

Mutation defaults false. The only variables are `CI_EXECUTION_MODE` and `CI_LINUX_RUNS_ON_JSON`, with selected visibility and explicit positive unique repository IDs.

Hosted and self-hosted labels must be nonempty, unique, whitespace-free, valid, and non-overlapping. For `build-server` or `hold`, the broker publishes `ci-capacity-hold-no-runner` rather than an invalid empty runner list. Workflows must gate on mode:

```yaml
if: vars.CI_EXECUTION_MODE == 'hosted' || vars.CI_EXECUTION_MODE == 'self-hosted'
runs-on: ${{ fromJSON(vars.CI_LINUX_RUNS_ON_JSON || '["ubuntu-latest"]') }}
```

## Promotion stages

### Stage 0 — merged but inert

- hosted contract and Rust tests pass;
- prerequisites may reconcile;
- controller, scale sets, broker, and mirror execution remain manual/digest/credential gated;
- no required check targets `sonus-ci`.

### Stage 1 — AWS registration smoke

- controller/CRD posture recorded;
- `sonus-aws` and ARC App verified;
- AWS scale set registers;
- manual smoke proves non-root UID, no host sockets, no Kubernetes token, bounded workspace, tools, and one-job teardown.

### Stage 2 — Hetzner active-active and failover

- `sonus-hetzner` and ARC App verified;
- Hetzner registers the same scale-set name;
- both clouds acquire manual jobs;
- pausing AWS proves Hetzner continues without workflow edits.

### Stage 3 — parity

Run hosted, AWS ARC, and Hetzner ARC on the same commit for Rust, Node, Dart/Flutter, and browser checks. Compare exit codes, test counts, artifacts, caches, timeouts, cancellation, environment assumptions, and required-check conclusions.

### Stage 4 — selected routing and continuity

- build, scan, attest, publish, and digest-pin `gha-capacity-broker-rs` and `gha-clone-server-rs`;
- reconcile all three App secrets without printing values;
- deploy with mutation and execution disabled;
- set `selfHostedReady=true` after stages 1–3;
- enable selected-repository mutation;
- verify 75/90/100 and billing-failure behavior;
- enable failure-only, deduplicated fixed-profile continuity;
- migrate required checks gradually.

## Failure semantics

- Hosted allocation failure before steps: capacity failure, not test failure.
- One ARC cluster unavailable: the other continues acquisition.
- Billing unavailable or schema drift: explicit ARC-or-hold policy.
- GitHub control plane unavailable: ARC cannot acquire new jobs; only independent reviewed profiles continue.
- Runner or App compromise: pause scale sets, disable mutation/dispatch, rotate affected keys, preserve evidence.

## Rollback

1. Disable broker mutation and mirror execution.
2. Restore hosted mode only when funded hosted capacity is verified; otherwise hold.
3. Pause both scale-set Applications.
4. Remove or retain controllers only through upstream procedures; do not strand CRDs.
5. Disable continuity webhooks and fixed-profile dispatch.
6. Rotate affected App keys.
7. Preserve workflow history, logs, artifacts, decisions, and parity evidence.

## Acceptance evidence

Completion requires current-month gross and net Actions minutes, controller/CRD ownership, runner-group restrictions, App inventories without values, three ExternalSecrets Ready in both clouds, AWS/Hetzner smokes and failover, hosted/AWS/Hetzner parity, fixed-profile continuity E2E, immutable image digests/SBOM/provenance, specialized-lane decisions, mutation/webhook audit, rollback drill, and proof that public-fork workflows cannot reach self-hosted or build-server capacity.
