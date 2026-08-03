# Sonus Auris self-hosted GitHub Actions runners

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

On July 27, 2026 the Sonus Auris organization exhausted all 2,000 included
GitHub-hosted Actions minutes. GitHub-hosted jobs then failed before runner
setup, with no job steps or logs. Increasing the Actions budget or waiting for
the August 1 reset is still required for macOS/iOS jobs; Linux self-hosted
runners reduce recurring hosted-minute use but cannot replace Apple hardware.

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

## Proposed lanes

| Label | Isolation | Intended jobs | Explicitly excluded |
| --- | --- | --- | --- |
| `sonus-ci` | non-privileged ephemeral pod | Rust, Node, Python, Dart/Flutter analyze/test/web/Linux builds, package-managed browser tests | Docker service containers, Android emulator/KVM, iOS/macOS |
| `sonus-ci-dind` | separately reviewed ephemeral pod with privileged Docker-in-Docker sidecar | trusted first-party jobs requiring service containers or image builds | fork PRs, Android emulator/KVM, iOS/macOS |

Start with `sonus-ci`. Add `sonus-ci-dind` only after its threat model,
NetworkPolicy, resource quota, and trusted-workflow restrictions are reviewed.
Do not mount the host Docker socket.

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
