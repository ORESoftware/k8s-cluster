# Sonus Auris ARC and CI failover plan

Status: **implementation-ready, activation gated**. This change adds reviewable code and GitOps declarations, but does not claim runner registration, parity, or required-check migration before the operator gates are satisfied.

Linear: `DEN-381` (durable self-hosted runners), `DEN-1549` (capacity broker and AWS/Hetzner failover), `DEN-378` (hosted-minute incident), and `DEN-27` (credential rotation).

## Problem and evidence

On July 27, 2026 the private `github.com/sonus-auris` organization recorded exhaustion of its 2,000 included GitHub Actions minutes. A representative later run (`30566670766`, job `90952795806`) failed before any workflow step existed and exposed no downloadable job log. That is runner-allocation evidence; it does not certify or reject the product code.

Included minutes reset by billing period. The connected repository API cannot establish the exact current August balance, so this design does not infer a current numeric total from July. `gha-clone-server-rs` reads the current UTC year and month from the authorized organization billing endpoint and fails closed when that endpoint is unavailable.

Hosted capacity remains necessary for macOS/iOS, Windows, Android/KVM, signing, notarization, and other specialized workloads. The first resilience target is trusted Linux CI.

## Architecture decision

1. Use GitHub's official Actions Runner Controller and runner for normal GitHub Actions workflow/action compatibility.
2. Deploy ARC independently in AWS and Hetzner after a legacy-controller/CRD audit.
3. Register two organization-level scale sets with the same `runnerScaleSetName: sonus-ci` and distinct runner groups, `sonus-aws` and `sonus-hetzner`.
4. Keep general runner pods ephemeral, non-root, token-free, socket-free, hostPath-free, network-bounded, and resource-bounded.
5. Run one `gha-clone-server-rs` instance for one GitHub organization and one App installation. It reports current-month capacity and can reconcile only selected-repository routing variables after certification.
6. Preserve `dd-build-server` as the existing bounded CI/CD path for reviewed `run-profile` requests, artifacts, NATS, Postgres, image building, and controlled deploys. It is not an arbitrary GitHub workflow executor.

The `ORESoftware` namespace is a personal account, not an organization. Organization runner groups, organization billing, and organization variables cannot be attached to `ORESoftware/k8s-cluster`. Broker code remains in that repository, while the first organization-level deployment targets `sonus-auris`.

## Capability-separated lanes

Use separate scale sets by capability, not one privileged universal runner.

| Label | Initial purpose | Privilege posture |
| --- | --- | --- |
| `sonus-ci` | Rust, Node, Python, Dart, ordinary Flutter analysis/test/web/Linux builds, docs and repository-contract checks | non-root, no host sockets, no host mounts, no Kubernetes token, no KVM |
| `sonus-browser` | future browser-heavy checks | non-root browser image, bounded egress, no production credentials |
| `sonus-container` | future service-container or OCI-build jobs | separate reviewed rootless/isolated lane; never host Docker/containerd socket |
| `sonus-android` | future emulator/device integration | KVM/device-isolated nodes only |

macOS/iOS and Windows remain hosted or use dedicated hardware fleets.

## Controller and chart lifecycle

The repository contains older ARC scaffolding for another organization. The GitHub-supported scale-set implementation is a substantial redesign rather than a casual chart bump. Before syncing `0.14.2`:

1. inventory active ARC releases, namespaces, service accounts, and `actions.github.com` CRDs in each cluster;
2. decide whether an existing compatible controller can be generalized or a clean isolated controller is required;
3. follow upstream clean-install/upgrade guidance and avoid stranded incompatible CRDs;
4. keep controller and scale-set chart versions identical;
5. stage AWS first, then Hetzner.

The committed controller and scale-set Applications are intentionally manual. Only namespace/quota/NetworkPolicy/ExternalSecret prerequisites reconcile automatically.

## Authentication and secret delivery

### ARC registration App

Create a dedicated App installed on `sonus-auris` with the organization self-hosted-runner permissions required by ARC. Store App ID, installation ID, and private key at `dd/ci/github-apps/sonus-auris-arc` in AWS Secrets Manager. External Secrets materializes `sonus-auris-arc-github` in `arc-runners-sonus`.

### Capacity-broker App

Use a separate App with organization Administration read for enhanced billing usage and Variables write for selected-repository variables. Store it at `dd/ci/github-apps/sonus-auris-capacity-broker`. One deployment represents one organization and one installation; do not reuse an installation ID as cross-organization authority.

Never commit App keys, registration tokens, generated Secret values, or a PAT. Revoke and rotate the classic PAT pasted into chat under `DEN-27`.

## Runner images

The first registration smoke uses the pinned official runner image `2.334.0`. This minimizes custom-image uncertainty while validating ARC, runner groups, secrets, and cluster routing.

The existing purpose-built Sonus runner image remains an inert parity candidate. It extends the official runner and adds Java 17 plus compiler, desktop, and browser libraries while workflows install pinned toolchains through setup actions and lockfiles. Before promotion, build it from reviewed source, scan it, generate SBOM/provenance, publish an immutable digest, and prove tool installation and one-job teardown.

Do not preinstall cloud credentials or broad cloud CLIs merely because one workflow might need them. Split those jobs or add a separately reviewed lane.

## Scale-set defaults

Both clouds use:

```yaml
githubConfigUrl: https://github.com/sonus-auris
githubConfigSecret: sonus-auris-arc-github
runnerScaleSetName: sonus-ci
minRunners: 0
maxRunners: 4
```

AWS uses `runnerGroup: sonus-aws`; Hetzner uses `runnerGroup: sonus-hetzner`. GitHub distributes jobs by acquisition race. If one scale set is unavailable, the other continues acquiring the same `runs-on: sonus-ci` jobs without a workflow edit.

Each runner pod must have non-root identity, dropped capabilities, RuntimeDefault seccomp, no privilege escalation, explicit resource requests/limits, bounded emptyDir workspaces, no service-account token, and egress policy blocking metadata and private networks. Ephemeral runners must be destroyed after one job; stuck/offline runners and queue age must be observable.

## Capacity policy

The broker filters billing records to `product=Actions` and `unitType=minutes` for the current UTC month.

- 75%: warning.
- 90%: route opted-in trusted Linux jobs to certified ARC.
- 100%: do not assume hosted allocation will succeed.
- billing API unavailable + ARC certified: fail closed to `sonus-ci`.
- billing API unavailable + ARC unready: hold.
- hard stop + ARC unready + reviewed build-server path: report `build-server`; only workflows already written for an allowlisted profile may delegate.

Mutation defaults to false. The only organization variables are `CI_EXECUTION_MODE` and `CI_LINUX_RUNS_ON_JSON`, both with `visibility: selected` and explicit repository IDs.

## Workflow compatibility matrix

Do not migrate required checks before same-commit parity.

| Workflow need | Initial lane/decision |
| --- | --- |
| Markdown, Python, repository-contract and gitlink checks | `sonus-ci` pilot |
| Rust fmt/clippy/test/doc/package without services | `sonus-ci` pilot |
| Node/Dart package and downstream-consumer checks | `sonus-ci` pilot |
| Flutter analyze/test and non-emulator builds | `sonus-ci` after pinned Flutter/Java proof |
| Puppeteer/Playwright/Selenium public/development checks | `sonus-ci` smoke, then `sonus-browser` if needed |
| PostgreSQL or other service containers | hosted until `sonus-container` is reviewed |
| Docker/OCI builds | bounded build server or isolated rootless build lane |
| Android emulator/KVM | hosted until dedicated node/device isolation is accepted |
| macOS/iOS signing and notarization | hosted macOS or dedicated Apple fleet |
| Windows installer/signing | hosted Windows or dedicated Windows fleet |

## Promotion stages

### Stage 0 — merged but gated

- hosted contract and Rust tests pass;
- prerequisites may reconcile;
- controller, scale sets, broker policy, and broker deployment remain manual/digest gated;
- no required check targets `sonus-ci`.

### Stage 1 — AWS registration smoke

- legacy controller/CRD posture recorded;
- `sonus-aws` group and App installation verified;
- AWS scale set registers;
- manual workflow proves non-root UID, no Docker/containerd socket, no Kubernetes token, writable bounded workspace, required base tools, and one-job lifecycle.

### Stage 2 — Hetzner active-active and failover

- `sonus-hetzner` group and App installation verified;
- Hetzner registers the same `sonus-ci` scale-set name;
- both clouds acquire manual jobs;
- pausing AWS proves Hetzner continues acquisition without workflow changes.

### Stage 3 — parity

Run representative hosted and ARC jobs on the same commit for Rust, Node, Dart/Flutter analysis/test, and browser checks. Compare exit codes, test counts, artifacts, environment assumptions, and timing. Keep service-container, image-build, KVM, signing, macOS, and Windows jobs excluded.

### Stage 4 — selected routing

- build, scan, attest, publish, and digest-pin `gha-clone-server-rs`;
- deploy with mutation disabled;
- set `selfHostedReady=true` only after stages 1–3;
- enable selected-repository mutations;
- verify 75/90/100 threshold and billing-failure behavior;
- migrate required checks gradually.

## Failure semantics

- Hosted allocation failure before steps: record as capacity failure, not test failure.
- One ARC cluster unavailable: the other scale set continues acquisition.
- Billing endpoint unavailable: apply the explicit fail-closed readiness policy.
- GitHub control plane unavailable: ARC cannot acquire new GitHub jobs; only direct clients of reviewed `dd-build-server` profiles continue.
- Runner or credential compromise: pause scale sets, rotate the App key/installation, preserve evidence, and restore hosted routing.

## Rollback

1. Set `GHA_MUTATION_ENABLED=false`.
2. Restore `CI_LINUX_RUNS_ON_JSON` to `["ubuntu-latest"]` for selected repositories.
3. Pause both scale-set Applications and allow ephemeral jobs to terminate.
4. Remove or retain controllers only according to upstream uninstall/upgrade guidance; do not strand CRDs.
5. Rotate App material after suspected compromise.
6. Preserve workflow history, logs, artifacts, and parity evidence.

## Acceptance evidence

The implementation is complete only when it links:

- current-month numeric billing usage or documented enhanced-billing unavailability;
- controller/CRD ownership and version decision for AWS and Hetzner;
- runner-group repository/workflow access restrictions;
- App permission inventories without secrets;
- ExternalSecret Ready evidence in both clusters;
- AWS smoke, Hetzner smoke, and failover proof;
- hosted-vs-ARC parity for representative Rust/Node and browser/Flutter jobs;
- immutable image digests, SBOMs, provenance, and vulnerability reports;
- explicit disposition for service containers, Android/KVM, macOS/iOS, and Windows;
- variable mutation audit, rollback drill, and capacity/queue/authentication alerts;
- evidence that no public-fork workflow can reach the self-hosted lane.
