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
This directory contains the shared security base and reviewable, inert templates
for organization-scoped, ephemeral Actions Runner Controller (ARC) scale sets
for `github.com/sonus-auris`.

The generic `*.template.*` files remain outside `remote/argocd/apps/`: merging a
template does not register runners. Provider-specific, manual-gated Application
declarations are now rendered from:

- `remote/argocd/clusters/aws/gha-ci.applications.yaml`;
- `remote/argocd/clusters/hetzner/gha-ci.applications.yaml`.

Those Applications create shared prerequisites automatically, but the actual
scale-set Applications have no automated sync policy. Promotion into an active
runner remains an explicit operator action after the GitHub runner group,
ExternalSecret, immutable image, and smoke evidence exist.

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
The current exact minute and budget state is deliberately not guessed. The
capacity audit in `gha-clone-server-rs` reads the authorized organization usage
and budget APIs. Its scheduled cluster job is suspended and mutation-disabled
until the capacity GitHub App has organization Administration read permission.

## Active-active AWS and Hetzner contract

GitHub documents active-active ARC failover as two clusters using the same
scale-set name and distinct runner groups. This repository uses:

| Provider | Runner group | Scale-set name | Initial bounds |
| --- | --- | --- | --- |
| AWS | `sonus-aws` | `sonus-ci` | `minRunners: 0`, `maxRunners: 4` |
| Hetzner | `sonus-hetzner` | `sonus-ci` | `minRunners: 0`, `maxRunners: 4` |

Create both groups in **Organization Settings → Actions → Runner groups** and
restrict them to the first trusted private repositories before syncing either
scale set. With both online, assignment is a race; if one cluster is down, the
other continues acquiring jobs.
Linear: `DEN-381`; shared capacity broker: `DEN-1549`; hosted-capacity incident: `DEN-378`.

This directory contains the GitOps prerequisites, inert examples, and activation contract for organization-scoped, ephemeral GitHub Actions Runner Controller (ARC) capacity for `github.com/sonus-auris`.

The production design is active-active across AWS and Hetzner:

- both clouds expose the same workflow target, `sonus-ci`;
- AWS assigns that scale set to runner group `sonus-aws`;
- Hetzner assigns it to runner group `sonus-hetzner`;
- controller and scale-set charts are pinned together at ARC `0.14.2`;
- the first-registration runner image is pinned at `2.334.0`;
- controller and scale-set Argo CD Applications remain manual until the controller/CRD audit, runner groups, App credentials, and smoke checks are complete.

Merging these files does **not** by itself register runners. The cloud overlays can reconcile the namespace, quotas, NetworkPolicy, and ExternalSecrets. An operator must deliberately sync each controller and scale-set Application after satisfying the rollout gates below.

## Why this exists

On July 27, 2026 the Sonus Auris organization exhausted its recorded 2,000 included GitHub-hosted Actions minutes. Representative jobs failed before runner setup, with no job steps or downloadable logs. That is runner-allocation evidence rather than a product test failure. Included minutes reset by billing period, so the July incident is not proof of the current August balance.

Self-hosted Linux capacity reduces dependency on hosted Linux minutes, but GitHub remains the workflow control plane. It also does not replace Apple hardware, Windows, Android/KVM, signing, or other specialized lanes.

## Compatibility and capacity boundaries

GitHub's official ARC controller and runner implement the runner scale-set protocol and execute normal trusted GitHub Actions workflow/action steps. `gha-clone-server-rs` is a per-organization capacity and routing broker; it is not a clone of GitHub's proprietary workflow service. It reads current-month billing usage, evaluates a reviewed policy, and may reconcile selected-repository routing variables after parity certification.

The existing `dd-build-server` remains a separate bounded CI/CD path for operator-reviewed `run-profile` requests. It retains its own repository, profile, image, namespace, authentication, deployment, NATS, and persistence controls. Neither the broker nor this runner design accepts arbitrary shell commands outside the normal GitHub runner boundary.

## Proposed lanes

Use separate ephemeral scale sets by capability rather than one privileged universal runner.

| Label | Isolation | Intended jobs | Explicitly excluded |
| --- | --- | --- | --- |
| `sonus-ci` | non-privileged ephemeral pod | Rust, Node, Python, Dart/Flutter analyze/test/web/Linux builds, package-managed browser tests | Docker service containers, Android emulator/KVM, iOS/macOS |
| `sonus-ci-dind` | separately reviewed ephemeral pod with privileged Docker-in-Docker sidecar | trusted first-party jobs requiring service containers or image builds | fork PRs, Android emulator/KVM, iOS/macOS |

Start with `sonus-ci`. Add `sonus-ci-dind` only after its threat model,
# Sonus Auris self-hosted GitHub Actions runners

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
| `sonus-ci` | non-privileged ephemeral pod | Rust, Node, Python, Dart/Flutter analyze/test/web/Linux builds, Chromium browser tests | Docker service containers, Android emulator/KVM, iOS/macOS |
| `sonus-ci-dind` | ephemeral pod with a privileged Docker-in-Docker sidecar | trusted first-party jobs requiring Postgres/service containers or container builds | fork PRs, Android emulator/KVM, iOS/macOS |
| `sonus-ci` | non-privileged ephemeral pod | Rust, Node, Python, Dart/Flutter analysis/test/web/Linux builds, docs, package and repository-contract checks | public forks, Docker/service containers, Android emulator/KVM, signing, macOS, Windows |
| `sonus-browser` | future non-root browser image | Puppeteer, Playwright, Selenium against public/development targets | production credentials, privileged containers, public forks |
| `sonus-container` | future isolated build/service lane | service-container or OCI jobs with a separate threat model | host Docker/containerd sockets |
| `sonus-android` | future KVM/device-isolated lane | Android emulator and device integration | general-purpose nodes |

Start with `sonus-ci`. Never mount host sockets, broad hostPath volumes, node credentials, production kubeconfig, cloud credentials, or a Kubernetes service-account token into the general lane.

## Files

- `base/`: namespace, ExternalSecret, ResourceQuota, LimitRange, and
  public-HTTPS-only runner NetworkPolicy used in both provider clusters.
- `sonus-ci-runner-set.application.template.yaml`: generic, opt-in template;
  it intentionally retains `REPLACE_RUNNER_GROUP` and `REPLACE_IMAGE_DIGEST`.
- `sonus-arc-github.externalsecret.template.yaml`: original inert AWS mapping,
  retained as a review fixture.
- `sonus-ci-smoke.workflow.template.yml`: manual, read-only first-registration
  proof for a trusted Sonus repository.
- `../../../deployments/sonus-auris-ci-runner/Dockerfile`: pinned ARC runner base
  plus Java 17 and Linux compiler, desktop, and browser libraries.

The cluster-wide ARC controller is shared rather than duplicated. The controller
and every runner-scale-set chart are pinned together at `0.14.2`; the runner
base is pinned at `2.334.0`.

## Required GitHub App secret

Create a GitHub App installed only on `sonus-auris` with GitHub's minimum
organization self-hosted-runner permissions. Store these fields under
`dd/ci/github-apps/sonus-auris-arc` and let External Secrets materialize
`sonus-auris-arc-github` in `arc-runners-sonus`:
| `sonus-ci` | non-privileged ephemeral pod | Rust, Node, Python, Dart/Flutter analysis/test/web/Linux builds, docs, package and repository-contract checks | public forks, Docker/service containers, Android emulator/KVM, signing, macOS, Windows |
| `sonus-browser` | future non-root browser image | Puppeteer, Playwright and Selenium against public/development targets | production credentials, privileged containers, public forks |
| `sonus-container` | future isolated build/service lane | jobs that genuinely require service containers or OCI builds | host Docker/containerd sockets; activation requires a separate threat model |
| `sonus-android` | future KVM/device-isolated lane | Android emulator and permission/recording integration | general-purpose nodes and the initial `sonus-ci` lane |

Start with `sonus-ci`. Do not mount `/var/run/docker.sock`, containerd sockets, broad `hostPath` volumes, node credentials, production kubeconfig, cloud credentials, or Kubernetes service-account tokens into general runner jobs.

## Files

- `base/`: active namespace, quota, LimitRange, default-deny runner NetworkPolicy, and ExternalSecret references shared by AWS and Hetzner.
- `sonus-ci-runner-set.application.template.yaml`: inert single-cluster custom-image example retained for image-parity work.
- `sonus-arc-github.externalsecret.template.yaml`: inert credential-field mapping example; names only, never secret values.
- `sonus-ci-smoke.workflow.template.yml`: manual, read-only registration and isolation proof for a trusted Sonus repository.
- `gha-clone-server-policy.configmap.template.yaml`: inert selected-repository routing policy.
- `gha-clone-server.deployment.template.yaml`: digest-gated, mutation-disabled broker deployment.
- `../../../clusters/aws/gha-ci.applications.yaml`: AWS controller and `sonus-ci` scale set.
- `../../../clusters/hetzner/gha-ci.applications.yaml`: Hetzner controller and `sonus-ci` scale set.
- `../../../deployments/sonus-auris-ci-runner/`: pinned custom runner image and image-validation contract.
- `../../../deployments/gha-clone-server-rs/`: tested capacity broker.

## Controller ownership and upgrade gate

The repository already contains older ARC scaffolding for another organization. The GitHub-supported scale-set implementation is not a minor in-place upgrade. Before syncing either new controller Application:

1. inventory running ARC controllers, namespaces, release names, and `actions.github.com` CRDs in each cluster;
2. confirm whether the existing controller can be safely reused or whether a clean isolated `0.14.2` installation is required;
3. follow the upstream clean-install/upgrade procedure rather than leaving incompatible CRDs behind;
4. keep controller and scale-set chart versions aligned;
5. prove the generated controller service account is `sonus-ci-arc-gha-rs-controller` before syncing the scale set.

The committed controller and scale-set Applications therefore have no automated sync policy.

## GitHub Apps and secrets

Use separate least-privilege Apps for runner registration and capacity reporting.

### ARC registration App

Install only on the `sonus-auris` organization with the organization self-hosted-runner permissions required by ARC. Store:

- `github_app_id`;
- `github_app_installation_id`;
- `github_app_private_key`.

Do not use a broad classic PAT and do not commit credentials or private keys.
The original inert ExternalSecret template continues to use namespace
`arc-runners` and store `aws-secrets`; it is not the active provider base.

## Promotion checklist

1. Review the ARC `0.14.2` controller/scale-set upgrade and confirm the shared
   controller is healthy in AWS and Hetzner.
2. Build, scan, and publish `sonus-auris-ci-runner`; replace the digest token in
   the chosen generic template or provider values before use.
3. Create `sonus-aws` and `sonus-hetzner` with selected-repository access only.
4. Reconcile `sonus-auris-arc-github` in both clusters without displaying values.
5. Manually sync the AWS runner scale set and run the credential-free smoke.
6. Manually sync Hetzner, temporarily stop AWS acquisition, and prove Hetzner
   obtains the same `sonus-ci` job.
7. Prove representative Rust, Node/browser, Dart/Flutter web, and Linux desktop
   jobs, including workspace teardown after every job.
8. Publish the two-binary `gha-clone-server` image, replace the audit image
   digest placeholder, and only then enable the suspended billing audit after
   its App returns usage and budget evidence. Keep
   `GHA_CAPACITY_MUTATION_ENABLED=false` during parity.
9. Do not migrate required checks until self-hosted output matches the hosted
   workflow. Keep a positive hosted Actions budget for macOS/iOS and emergencies.

## Security posture

- Ephemeral one-job runners, non-root, dropped capabilities, RuntimeDefault
  seccomp, bounded resources, emptyDir-only storage, no hostPath, and no
  Kubernetes service-account token.
- Trusted first-party repositories only. Public-fork pull requests and other
  untrusted code stay off cluster-adjacent self-hosted runners.
- No cluster credentials, cloud credentials, production kubeconfig, or internal
  service tokens in the image.
- Use OIDC or narrowly scoped short-lived credentials only in separately
  reviewed write-capable workflows.
- Preserve all existing checks. A self-hosted lane changes compute, not
  acceptance criteria.
- [`sonus-ci-runner-set.application.template.yaml`](sonus-ci-runner-set.application.template.yaml):
  opt-in Argo CD Application template for the non-privileged scale set.
- [`../../../deployments/sonus-auris-ci-runner/Dockerfile`](../../../deployments/sonus-auris-ci-runner/Dockerfile):
  pinned ARC runner base plus Node 22, Chromium, Java 17, and Linux build
  libraries used by Rust/Flutter/browser jobs.
- [`../../../deployments/sonus-auris-ci-runner/README.md`](../../../deployments/sonus-auris-ci-runner/README.md):
  image build, digest pinning, and validation contract.

The cluster-wide ARC controller should be generalized/reused rather than
installing a second controller. The existing controller and runner-set chart
versions must remain identical.

## Required GitHub App secret

Create a GitHub App installed only on the `sonus-auris` organization with the
minimum permissions GitHub requires for organization self-hosted runners. Store
these values in AWS Secrets Manager and synchronize them with External Secrets:

- `github_app_id`
- `github_app_installation_id`
- `github_app_private_key`

The resulting Kubernetes Secret name in namespace `arc-runners` is expected to
be `sonus-auris-arc-github`. Do not use a broad classic PAT and do not commit
credentials or private keys.

## Promotion checklist

1. Verify the ARC controller/chart release and runner base-image tag against
   GitHub's current release; pin the custom image by immutable digest.
2. Build, scan, and publish the runner image. Record its digest in the template.
3. Reconcile the GitHub App secret through External Secrets.
4. Add ResourceQuota, LimitRange, PodDisruptionBudget where applicable, and a
   default-deny NetworkPolicy that blocks cluster-internal destinations while
   allowing only required public package/GitHub endpoints and DNS.
5. Copy the reviewed Application template into `remote/argocd/apps/` only after
   all placeholders are replaced.
6. Confirm the scale set appears in **Organization Settings → Actions → Runners**.
7. Run a `workflow_dispatch` smoke job on `runs-on: sonus-ci` that performs no
   writes and prints no secrets.
8. Prove representative Rust, Node/browser, Dart/Flutter web, and Linux desktop
   jobs. Validate workspace teardown after every job.
9. Do not migrate required checks until the self-hosted output matches the
   hosted workflow. Keep a positive hosted Actions budget as the macOS/iOS and
   emergency fallback.

## Security posture

- Ephemeral runners only; `minRunners: 0` and a bounded `maxRunners`.
- Trusted first-party repositories only. Do not route untrusted fork code to
  self-hosted runners.
- Non-root runner container, dropped capabilities, seccomp RuntimeDefault,
  bounded CPU/memory/ephemeral storage, no hostPath mounts, no service-account
  token unless explicitly required.
- No cluster credentials, cloud credentials, production kubeconfig, or internal
  service tokens in the runner image.
- Jobs must use OIDC or narrowly scoped short-lived credentials where external
  writes are later authorized.
- Preserve all existing repository checks. A self-hosted lane changes compute,
  not acceptance criteria.

## Platform limits

Linux ARC runners cannot execute the iOS compile/sign/upload workflow. They also
cannot faithfully run the existing Android emulator job without a separately
reviewed KVM-capable lane. The immediate complete unblock therefore still
requires a positive GitHub-hosted Actions budget or the August 1, 2026 minute
reset.

## Rollback

Set both scale-set maxima to zero or pause the provider Applications, restore
`CI_LINUX_RUNS_ON_JSON` to `["ubuntu-latest"]` for adopted repositories, and
leave the controller in place until all ephemeral runners terminate. Disable
capacity mutation first. Do not bypass required checks or delete evidence.
The AWS Secrets Manager record is `dd/ci/github-apps/sonus-auris-arc`; External Secrets materializes `sonus-auris-arc-github` in `arc-runners-sonus`.

### Capacity-broker App

Install separately on `sonus-auris` with organization Administration read for billing usage and Variables write for the two selected-repository routing variables. Store it at `dd/ci/github-apps/sonus-auris-capacity-broker`; External Secrets materializes `sonus-auris-gha-capacity-broker`.

Never commit App keys, registration tokens, decoded Secret manifests, a classic PAT, or credential-bearing command output. The classic PAT pasted into chat must be revoked and rotated under `DEN-27` and is not used by these manifests or services.

## Runner image integrity

The active first-registration scale sets use the pinned official runner image to minimize bootstrap variables. The custom Sonus image remains inert until it is built, scanned, and pinned by digest.

The custom image is intended to supply Java 17 plus the compiler, Linux desktop, and browser libraries needed by Rust, Flutter Linux/web, and package-managed Playwright/Puppeteer/Selenium jobs. Repository workflows continue to select exact Node, Python, Rust, Dart, Flutter, Java, and browser versions through pinned setup actions and lockfiles.

Before custom-image promotion:

1. build from the reviewed Dockerfile;
2. pin the official ARC runner version;
3. scan for actionable high/critical vulnerabilities;
4. generate an SBOM and provenance attestation;
5. push an immutable version and record the digest;
6. replace `REPLACE_IMAGE_DIGEST` only in a reviewed activation change;
7. prove tool installation, workspace limits, and one-job teardown.

Do not deploy a mutable `latest` tag.

## Security posture

- Ephemeral one-job runners only; `minRunners: 0` and bounded `maxRunners`.
- Trusted private repositories and approved workflows only; no untrusted fork pull requests.
- Non-root execution, dropped Linux capabilities, `seccompProfile: RuntimeDefault`, no privilege escalation, and no host sockets or `hostPath` mounts.
- No runner Kubernetes service-account token.
- Explicit CPU, memory, ephemeral-storage, namespace quota, and workspace limits.
- Runner NetworkPolicy permits DNS and public HTTPS while blocking cloud metadata, loopback, carrier-grade NAT, and private address ranges.
- No production cloud credentials, cluster credentials, App private keys, or internal service tokens in the runner image.
- Use OIDC or narrowly scoped short-lived credentials for later reviewed external writes.
- Preserve all existing acceptance checks; changing compute must not weaken test criteria.

## Workflow compatibility matrix

Do not migrate a required check until hosted and self-hosted results match on the same commit.

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
| Flutter analyze/test and non-emulator web/Linux builds | `sonus-ci` after pinned toolchain proof |
| Puppeteer/Playwright/Selenium | initial credential-free smoke, then separate browser lane if needed |
| PostgreSQL or other service containers | keep hosted until `sonus-container` is reviewed |
| Docker/OCI builds | existing bounded build server or a separate rootless/isolated build lane; never host socket mounts |
| Android emulator/KVM | hosted or a future KVM-isolated lane |
| macOS/iOS compile, signing, notarization | hosted macOS or a dedicated Apple fleet |
| Windows installer/signing | hosted Windows or a dedicated Windows fleet |

## Activation checklist

1. Run the credential-free repository contract and Rust tests on GitHub-hosted Actions.
2. Audit legacy controllers and CRDs in AWS and Hetzner.
3. Create organization runner groups `sonus-aws` and `sonus-hetzner`, restricted to trusted repositories and approved workflows.
4. Create and install the ARC App; populate `dd/ci/github-apps/sonus-auris-arc`.
5. Confirm `sonus-auris-arc-github` is Ready in each cluster without printing values.
6. Manually sync AWS controller and scale set; confirm the label appears under organization runner settings.
7. Copy the smoke template to a trusted first-party Sonus repository and run it with `workflow_dispatch`.
8. Confirm non-root execution, no host sockets, no Kubernetes token, writable bounded workspace, and one-job pod teardown.
9. Manually sync Hetzner with the same scale-set name and its distinct runner group.
10. Pause AWS acquisition and prove Hetzner continues acquiring the same workflow target without workflow edits.
11. Run hosted-vs-ARC parity for representative Rust, Node, Dart/Flutter, and browser checks.
12. Build, scan, attest, publish, and digest-pin `gha-clone-server-rs`; keep mutation disabled initially.
13. Set `selfHostedReady=true` only after parity; enable selected-repository variable mutation and verify 75/90/100 threshold behavior.
14. Migrate required checks gradually while retaining funded hosted capacity for specialized platforms and emergencies.

## Rollback

- Disable broker mutation and restore `CI_LINUX_RUNS_ON_JSON` to `["ubuntu-latest"]` for selected repositories.
- Pause both runner-scale-set Applications and allow ephemeral jobs to finish or terminate.
- Follow upstream ARC uninstall/upgrade guidance before deleting controllers or CRDs.
- Rotate the affected GitHub App key/installation after suspected compromise.
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

The rollout is complete only when it links:

- current-month numeric Actions usage from an authorized billing endpoint, or documented enhanced-billing unavailability;
- controller/CRD ownership and version decision for both clusters;
- runner-group repository/workflow access policy;
- GitHub App permission inventory without secrets;
- successful ExternalSecret reconciliation in AWS and Hetzner;
- AWS smoke, Hetzner smoke, and one-cluster failover proof;
- hosted-vs-self-hosted parity for representative Rust/Node and browser/Flutter checks;
- immutable image digests, SBOMs, provenance, and vulnerability reports;
- explicit disposition for service containers, Android/KVM, macOS/iOS, and Windows;
- rollback drill and recurring capacity/queue/authentication alerts;
- evidence that no public-fork job can reach the self-hosted lane.
Completion requires current-month gross and net Actions minutes, controller/CRD ownership, runner-group allowlists, App inventories without values, all three ExternalSecrets Ready in both clouds, AWS and Hetzner smokes, a failover proof, hosted-vs-ARC parity, fixed-profile continuity E2E, immutable image digests/SBOM/provenance, specialized-lane dispositions, rollback evidence, and proof that no public-fork workflow reaches self-hosted capacity.
