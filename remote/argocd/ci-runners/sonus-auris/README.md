# Sonus Auris active-active GitHub Actions runners

Linear: `DEN-381`; capacity and routing: `DEN-1549`; independent continuity: `DEN-1550`; hosted-capacity incident: `DEN-378`; credential rotation: `DEN-27`.

This directory contains GitOps prerequisites, inert templates, and activation contracts for organization-scoped ephemeral Actions Runner Controller (ARC) capacity for `github.com/sonus-auris`.

The production design is active-active across AWS and Hetzner:

- both clouds expose `runs-on: sonus-ci`;
- AWS assigns the scale set to runner group `sonus-aws`;
- Hetzner assigns it to runner group `sonus-hetzner`;
- controller and scale-set charts are pinned at ARC `0.14.2`;
- the initial runner image is pinned at `2.334.0`;
- controller and scale-set Applications remain manual until controller/CRD audit, runner groups, three Apps, and smoke checks are complete.

Merging these files does **not** register a runner or prove parity. The cloud overlays may reconcile namespace, quotas, NetworkPolicy, and ExternalSecrets. An operator must deliberately sync each controller and scale-set Application after satisfying the gates below.

## Why this exists

On July 27, 2026, Sonus Auris recorded exhaustion of a 2,000-minute hosted Actions allowance. Representative jobs failed before runner setup and exposed no job steps or logs. That is capacity-allocation evidence, not a product test failure.

Included usage resets by billing period. Recent August 3 runs in `ORESoftware/k8s-cluster` prove GitHub-hosted Actions are not globally exhausted, while the connected repository API still cannot expose the exact current Sonus numeric balance. The capacity broker queries the current month and fails closed when the request, authorization, or public-preview response contract is unavailable.

## Compatibility and continuity boundaries

- Official ARC and the official runner provide normal trusted Linux GitHub Actions workflow/action compatibility.
- `gha-capacity-broker-rs` selects hosted, ARC, build-server, or hold mode. It is not a clone of GitHub's proprietary workflow service.
- `gha-clone-server-rs` is the independent fail-closed planner and fixed-profile dispatcher. It does not promise arbitrary workflow-YAML parity.
- `dd-build-server` is the pre-existing bounded CI/CD execution path for reviewed run profiles, artifacts, NATS/Postgres-backed jobs, container builds, and controlled deploys.

Neither custom service accepts arbitrary shell commands, arbitrary repositories, or unsupported workflow semantics. Unsupported work fails closed.

## Capability-separated lanes

| Label | Isolation | Intended jobs | Explicitly excluded |
| --- | --- | --- | --- |
| `sonus-ci` | non-privileged ephemeral pod | Rust, Node, Python, Dart/Flutter analysis/test/web/Linux builds, docs, package and repository-contract checks | public forks, Docker/service containers, Android emulator/KVM, signing, macOS, Windows |
| `sonus-browser` | future non-root browser image | Puppeteer, Playwright, Selenium against public/development targets | production credentials, privileged containers, public forks |
| `sonus-container` | future isolated build/service lane | service-container or OCI jobs with a separate threat model | host Docker/containerd sockets |
| `sonus-android` | future KVM/device-isolated lane | Android emulator and device integration | general-purpose nodes |

Start with `sonus-ci`. Never mount host sockets, broad hostPath volumes, node credentials, production kubeconfig, cloud credentials, or a Kubernetes service-account token into the general lane.

## Files

- `base/`: namespace, quota, LimitRange, runner NetworkPolicy, ARC App secret, capacity-mutation App secret, and billing-read App secret.
- `sonus-ci-runner-set.application.template.yaml`: inert custom-image example.
- `sonus-arc-github.externalsecret.template.yaml`: inert ARC credential mapping example.
- `sonus-ci-smoke.workflow.template.yml`: manual read-only registration and isolation proof.
- `gha-capacity-broker-policy.configmap.template.yaml`: inert selected-repository routing policy.
- `gha-capacity-broker.deployment.template.yaml`: digest-gated, mutation-disabled broker deployment with distinct billing and mutation App mounts.
- `../../../clusters/aws/gha-ci.applications.yaml`: AWS controller and `sonus-ci` scale set.
- `../../../clusters/hetzner/gha-ci.applications.yaml`: Hetzner controller and `sonus-ci` scale set.
- `../../../deployments/sonus-auris-ci-runner/`: pinned custom runner image.
- `../../../deployments/gha-capacity-broker-rs/`: billing-aware lane selection.
- `../../../deployments/gha-clone-server-rs/`: independent reviewed-workflow continuity planner.

## Controller ownership and upgrade gate

Before syncing ARC `0.14.2`:

1. inventory running controllers, namespaces, releases, service accounts, and `actions.github.com` CRDs in each cluster;
2. decide whether a compatible controller can be shared or a clean isolated install is required;
3. follow upstream clean-install/upgrade guidance and avoid stranded CRDs;
4. keep controller and scale-set chart versions aligned;
5. stage AWS, then Hetzner.

The committed controller and scale-set Applications have no automated sync policy.

## Three GitHub Apps

### ARC registration App

Install a dedicated App on `sonus-auris` with the organization self-hosted-runner permissions required by ARC. Store App ID, installation ID, and private key at `dd/ci/github-apps/sonus-auris-arc`. External Secrets materializes `sonus-auris-arc-github`, consumed only by ARC.

### Billing-read App

Install a second App with organization `Administration: read`. Store it at `dd/ci/github-apps/sonus-auris-billing`. External Secrets materializes `sonus-auris-gha-billing`, mounted only at `/var/run/gha-billing-app/github_app_private_key`.

The broker uses this App only for:

```text
GET /organizations/sonus-auris/settings/billing/usage/summary
```

The current public-preview response reports `grossQuantity`, `discountQuantity`, and `netQuantity`. Capacity policy uses gross Actions minutes; billable/cost telemetry uses net Actions minutes.

### Capacity-mutation App

Install a third least-privilege App for the two selected-repository Actions variables. Store it at `dd/ci/github-apps/sonus-auris-capacity-broker`. External Secrets materializes `sonus-auris-gha-capacity-broker`, mounted only at `/var/run/gha-mutation-app/github_app_private_key`.

The billing and mutation App installations and key paths must be distinct; the broker rejects shared authority at startup. Never expose App private keys or minted installation tokens through environment values, logs, workflow inputs, metrics, or responses.

The PAT pasted into chat is not used. Revoke and rotate it under DEN-27.

## Runner image integrity

The first registration smoke uses official runner image `2.334.0`. The custom Sonus image remains inert until it is built, scanned, SBOMed, attested, and pinned by digest.

Repository workflows continue to install exact Node, Python, Rust, Dart, Flutter, Java, and browser versions through pinned setup actions and lockfiles. Do not deploy mutable `latest` tags or preinstall broad cloud credentials.

## Security posture

- One-job ephemeral runners; `minRunners: 0`, bounded `maxRunners`.
- Trusted private repositories and approved workflows only; no untrusted fork PRs.
- Non-root, dropped capabilities, RuntimeDefault seccomp, no privilege escalation, no host sockets or hostPath.
- No Kubernetes service-account token.
- Explicit CPU, memory, ephemeral-storage, namespace, and workspace limits.
- DNS/public-HTTPS egress only, with metadata and private address ranges blocked.
- No production cloud credentials, cluster credentials, App keys, or billing tokens in runner images.
- Preserve all acceptance checks; changing compute must not weaken criteria.

## Capacity variables and safe workflow adoption

The broker mutates only selected-repository variables:

- `CI_EXECUTION_MODE`
- `CI_LINUX_RUNS_ON_JSON`

Participating jobs must gate execution mode:

```yaml
jobs:
  test:
    if: vars.CI_EXECUTION_MODE == 'hosted' || vars.CI_EXECUTION_MODE == 'self-hosted'
    runs-on: ${{ fromJSON(vars.CI_LINUX_RUNS_ON_JSON || '["ubuntu-latest"]') }}
    steps:
      - run: ./ci/test.sh
```

For `build-server` and `hold`, the broker writes `ci-capacity-hold-no-runner`, a deliberately nonexistent label, and the mode gate skips the GitHub job. An allowlisted fixed profile may be dispatched independently by the continuity service. Do not migrate required checks until this behavior and hosted-vs-ARC parity are proven.

## Workflow compatibility matrix

| Workflow need | Initial lane/decision |
| --- | --- |
| Markdown, Python, repository-contract and gitlink checks | `sonus-ci` pilot |
| Rust fmt/clippy/test/doc/package without services | `sonus-ci` pilot |
| Node/Dart package and downstream-consumer checks | `sonus-ci` pilot |
| Flutter analyze/test and non-emulator web/Linux builds | after pinned toolchain proof |
| Puppeteer/Playwright/Selenium | credential-free smoke, then separate browser lane if required |
| PostgreSQL/service containers | hosted until `sonus-container` is reviewed |
| Docker/OCI builds | existing bounded build server or isolated rootless lane |
| Android emulator/KVM | hosted or dedicated device-isolated lane |
| macOS/iOS signing/notarization | hosted macOS or Apple fleet |
| Windows installer/signing | hosted Windows or Windows fleet |

## Activation checklist

1. Run credential-free contract and Rust tests on GitHub-hosted Actions.
2. Audit legacy controllers and CRDs in AWS and Hetzner.
3. Create `sonus-aws` and `sonus-hetzner`, restricted to trusted repositories and workflows.
4. Create/install the ARC registration, billing-read, and capacity-mutation Apps.
5. Confirm all three ExternalSecrets are Ready without printing values.
6. Manually sync AWS controller/scale set and run the smoke workflow.
7. Confirm non-root execution, no host sockets, no Kubernetes token, bounded workspace, and one-job teardown.
8. Sync Hetzner and prove it acquires the same label.
9. Pause AWS acquisition and prove Hetzner failover without workflow edits.
10. Run hosted-vs-AWS-vs-Hetzner parity for representative Rust, Node, Dart/Flutter, and browser checks.
11. Build, scan, attest, publish, and digest-pin broker and continuity images.
12. Keep mutation and continuity execution disabled during comparison.
13. Set `selfHostedReady=true`, enable selected-repository mutation, and verify 75/90/100 and billing-failure behavior.
14. Migrate required checks gradually.

## Rollback

- Disable broker mutation and continuity execution.
- Restore hosted mode only when funded hosted capacity is confirmed; otherwise use hold.
- Pause both scale-set Applications and allow ephemeral runners to terminate.
- Follow upstream ARC uninstall guidance before removing controllers or CRDs.
- Rotate affected App keys after suspected compromise.
- Preserve workflow history, logs, artifacts, and parity evidence.

## Acceptance evidence

Completion requires current-month gross and net Actions minutes, controller/CRD ownership, runner-group allowlists, App inventories without values, all three ExternalSecrets Ready in both clouds, AWS and Hetzner smokes, a failover proof, hosted-vs-ARC parity, fixed-profile continuity E2E, immutable image digests/SBOM/provenance, specialized-lane dispositions, rollback evidence, and proof that no public-fork workflow reaches self-hosted capacity.
