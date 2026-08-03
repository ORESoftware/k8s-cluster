# Sonus Auris active-active GitHub Actions runners

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

GitHub's official ARC controller and runner implement the runner scale-set protocol and execute normal trusted GitHub Actions workflow/action steps. `gha-capacity-broker-rs` is a per-organization capacity and routing broker; it is not a clone of GitHub's proprietary workflow service. It reads current-month billing usage, evaluates a reviewed policy, and may reconcile selected-repository routing variables after parity certification.

The existing `dd-build-server` remains a separate bounded CI/CD path for operator-reviewed `run-profile` requests. It retains its own repository, profile, image, namespace, authentication, deployment, NATS, and persistence controls. Neither the broker nor this runner design accepts arbitrary shell commands outside the normal GitHub runner boundary.

## Proposed lanes

Use separate ephemeral scale sets by capability rather than one privileged universal runner.

| Label | Isolation | Intended jobs | Explicitly excluded |
| --- | --- | --- | --- |
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
- `gha-capacity-broker-policy.configmap.template.yaml`: inert selected-repository routing policy.
- `gha-capacity-broker.deployment.template.yaml`: digest-gated, mutation-disabled broker deployment.
- `../../../clusters/aws/gha-ci.applications.yaml`: AWS controller and `sonus-ci` scale set.
- `../../../clusters/hetzner/gha-ci.applications.yaml`: Hetzner controller and `sonus-ci` scale set.
- `../../../deployments/sonus-auris-ci-runner/`: pinned custom runner image and image-validation contract.
- `../../../deployments/gha-capacity-broker-rs/`: tested capacity broker.

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
12. Build, scan, attest, publish, and digest-pin `gha-capacity-broker-rs`; keep mutation disabled initially.
13. Set `selfHostedReady=true` only after parity; enable selected-repository variable mutation and verify 75/90/100 threshold behavior.
14. Migrate required checks gradually while retaining funded hosted capacity for specialized platforms and emergencies.

## Rollback

- Disable broker mutation and restore `CI_LINUX_RUNS_ON_JSON` to `["ubuntu-latest"]` for selected repositories.
- Pause both runner-scale-set Applications and allow ephemeral jobs to finish or terminate.
- Follow upstream ARC uninstall/upgrade guidance before deleting controllers or CRDs.
- Rotate the affected GitHub App key/installation after suspected compromise.
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

## Capacity broker versus workflow mirror

`gha-capacity-broker-rs` owns billing/capacity policy and reviewed repository
routing. `gha-clone-server-rs` remains a separate, fail-closed independent
workflow mirror backed by fixed `dd-build-server` profiles. Neither service
silently substitutes unsupported GitHub Actions semantics.
