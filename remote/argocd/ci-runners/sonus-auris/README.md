# Sonus Auris self-hosted GitHub Actions runners

This directory is **reviewable, inert scaffolding** for organization-scoped,
ephemeral Actions Runner Controller (ARC) scale sets for
`github.com/sonus-auris`.

It is intentionally outside `remote/argocd/apps/`: merging this directory does
not register runners or create Kubernetes resources. Promotion into the active
Argo CD app directory requires the prerequisites and smoke evidence below.

## Why this exists

On July 27, 2026 the Sonus Auris organization exhausted all 2,000 included
GitHub-hosted Actions minutes. GitHub-hosted jobs then failed before runner
setup, with no job steps or logs. Increasing the Actions budget or waiting for
the August 1 reset is still required for macOS/iOS jobs; Linux self-hosted
runners reduce recurring hosted-minute use but cannot replace Apple hardware.

## Proposed lanes

| Label | Isolation | Intended jobs | Explicitly excluded |
| --- | --- | --- | --- |
| `sonus-ci` | non-privileged ephemeral pod | Rust, Node, Python, Dart/Flutter analyze/test/web/Linux builds, Chromium browser tests | Docker service containers, Android emulator/KVM, iOS/macOS |
| `sonus-ci-dind` | ephemeral pod with a privileged Docker-in-Docker sidecar | trusted first-party jobs requiring Postgres/service containers or container builds | fork PRs, Android emulator/KVM, iOS/macOS |

Start with `sonus-ci`. Add `sonus-ci-dind` only after the threat model,
NetworkPolicy, resource quota, and trusted-workflow restrictions are reviewed.
Do not mount the host Docker socket.

## Files

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
