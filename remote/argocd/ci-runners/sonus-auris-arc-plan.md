# Sonus Auris ARC runner plan

Status: **design and rollout gate only**. This document deliberately does not add a live Argo CD `Application`, GitHub credential secret, runner image reference, or required-check migration.

Linear: `DEN-381` (durable self-hosted runners) and `DEN-378` (immediate hosted-minute exhaustion).

## Why this exists

On July 27, 2026 the private `github.com/sonus-auris` organization exhausted its 2,000 included GitHub Actions minutes. GitHub-hosted jobs then failed before runner setup, so unrelated repositories showed zero-step failures with no logs. Raising the Actions budget or waiting for the August 1 reset is the immediate unblock; a self-hosted lane is the longer-term resilience path.

This repository already contains a working ARC design for another organization:

- [`README.md`](README.md)
- [`../apps/canonical-ci-arc-controller.application.yaml`](../apps/canonical-ci-arc-controller.application.yaml)
- [`../apps/canonical-browser-runner-set.application.yaml`](../apps/canonical-browser-runner-set.application.yaml)
- [`../../deployments/canonical-ci-runner/Dockerfile`](../../deployments/canonical-ci-runner/Dockerfile)

Sonus Auris should reuse the proven `gha-runner-scale-set` model, but it must not copy the canonical-cloud names, credentials, registry assumptions, or browser-only image blindly.

## Safety decision

Use **separate ephemeral scale sets by capability**, not one privileged universal runner.

| Proposed label | Initial purpose | Privilege posture |
| --- | --- | --- |
| `sonus-ci` | Rust, Node, Python, Dart, ordinary Flutter analysis/builds, docs and repository-contract checks | non-root, no Docker socket, no host mounts, no KVM |
| `sonus-browser` | Puppeteer, Playwright and Selenium against public/development targets | non-root Chromium image, no credentials beyond checkout token, bounded egress |
| `sonus-container` | Workflows that genuinely require service containers or image builds | separate opt-in lane; choose Kubernetes container mode or isolated rootless buildkit after a threat review |
| `sonus-android` | Android emulator and recording/permission integration | deferred until KVM/device isolation is designed; keep GitHub-hosted fallback initially |

Do not grant `sonus-ci` or `sonus-browser` access to `/var/run/docker.sock`, broad `hostPath` mounts, the Kubernetes API, node credentials, cloud credentials, or production secrets.

## Controller reuse

The existing `gha-runner-scale-set-controller` installation is cluster-scoped infrastructure even though its current Argo CD application name is canonical-specific. Before adding a Sonus scale set:

1. verify the installed controller watches the intended runner namespaces;
2. decide whether to rename/generalize the Argo CD application without disrupting the existing canonical scale set;
3. keep the controller chart version pinned and review upstream release notes before any upgrade;
4. deploy only one compatible controller unless a documented isolation requirement justifies another.

The first Sonus change should add a scale set only after this controller-ownership decision is recorded.

## GitHub authentication

Register the runner scale sets against `https://github.com/sonus-auris` with a dedicated GitHub App.

- Use GitHub's documented minimum ARC permissions for organization runners; record the exact repository and organization permission set in the implementation PR.
- Install the app only on the Sonus Auris organization and only on repositories that need the runner during the pilot.
- Store app ID, installation ID and private key in AWS Secrets Manager and synchronize them through External Secrets.
- Never commit app credentials, a PAT, registration tokens, generated Kubernetes `Secret` YAML, or decoded secret values.
- A classic PAT may be used only as a short-lived, explicitly approved diagnostic fallback and must be revoked after the App path is working.

Suggested secret name: `sonus-auris-arc-github` in a dedicated runner namespace such as `arc-sonus-runners`.

## Runner images

Create purpose-built, pinned images instead of using floating tags.

### `sonus-ci`

Start from a pinned official ARC runner image and add only the common private-repository toolchain:

- Git, CA certificates, curl and standard archive tools;
- Node 22;
- Python 3.12;
- a pinned Rust toolchain plus rustfmt and clippy, or allow the pinned `dtolnay/rust-toolchain` action to install into an ephemeral cache;
- Java 17 and the exact Flutter/Dart version used by Sonus CI;
- PostgreSQL client and other small clients needed by credential-free checks.

Do not preinstall cloud CLIs or production credentials merely because one workflow might use them. Split those jobs or add a separate reviewed image.

### `sonus-browser`

The existing Chromium image is a useful starting point. Publish a Sonus-owned immutable image and expose:

- `PLAYWRIGHT_CHROMIUM=/usr/bin/chromium`
- `CHROME_PATH=/usr/bin/chromium`
- `PUPPETEER_EXECUTABLE_PATH=/usr/bin/chromium`
- browser-download skip variables

Keep browser dependencies, fonts and Node pinned. Scan the image before promotion.

### Image integrity

- Build from a reviewed Dockerfile in this repository.
- Pin the ARC base image version and action/toolchain versions.
- Publish to a registry the cluster can read without embedding pull credentials in the image.
- Promote by immutable digest or an immutable release tag plus recorded digest.
- Generate an SBOM and run vulnerability scanning; document the patch cadence and emergency rebuild process.

## Scale-set defaults

Pilot values should be conservative:

```yaml
# Design sketch only — do not apply this block directly.
githubConfigUrl: https://github.com/sonus-auris
githubConfigSecret: sonus-auris-arc-github
runnerScaleSetName: sonus-ci
minRunners: 0
maxRunners: 2
```

Each pod must have:

- non-root user/group where supported;
- `allowPrivilegeEscalation: false`;
- dropped Linux capabilities;
- `seccompProfile: RuntimeDefault`;
- explicit CPU/memory requests and limits;
- ephemeral workspace and no persistent cross-job cache containing source or credentials;
- a bounded termination grace period;
- no service-account token mount unless a specific reviewed need exists;
- network policy allowing required GitHub/package endpoints while blocking cluster metadata and unrelated private services where practical.

Ephemeral runners must be torn down after one job. A failed cleanup or offline runner must be observable and alertable.

## Workflow compatibility matrix

Do not migrate a required check until its needs are proven on the correct lane.

| Workflow need | Initial lane/decision |
| --- | --- |
| Markdown, Python, repository-contract and gitlink checks | `sonus-ci` pilot |
| Rust fmt/clippy/test/doc/package without service containers | `sonus-ci` pilot |
| Node/Dart package and downstream-consumer checks | `sonus-ci` pilot |
| Flutter analyze/test and non-emulator builds | `sonus-ci` after pinned Flutter/Java proof |
| Puppeteer/Playwright/Selenium public-site tests | `sonus-browser` pilot |
| PostgreSQL or other service containers | keep hosted until `sonus-container` design is validated |
| Docker/OCI builds | isolated rootless build lane; never mount the host Docker socket into general runners |
| Android emulator/KVM | keep hosted until a dedicated node/device-isolation design is accepted |
| macOS/iOS signing and notarization | GitHub-hosted macOS or a separately managed macOS fleet; not this Kubernetes runner set |
| Windows installer/signing | GitHub-hosted Windows or a separately managed Windows fleet; not this Linux scale set |

## Rollout sequence

1. **Immediate recovery:** raise the Sonus Actions budget above $0 or wait for the August 1 included-minute reset; rerun all currently blocked checks unchanged.
2. **Controller audit:** document the existing ARC controller's namespace/watch/version posture.
3. **Credential setup:** create the Sonus GitHub App and External Secret path without exposing values.
4. **Image build:** add and scan pinned `sonus-ci` and `sonus-browser` images.
5. **Scale-set PR:** add Argo CD applications with `minRunners: 0`, low `maxRunners`, explicit resources and security contexts.
6. **Registration proof:** confirm the labels appear under Sonus Auris **Settings → Actions → Runners** and no runner is shared with another organization.
7. **Opt-in smoke:** add `workflow_dispatch`-only jobs in representative repositories; verify checkout, logs, teardown and redaction.
8. **Parity:** run the same commit on hosted and self-hosted lanes and compare results/artifacts.
9. **Selective migration:** move only compatible required checks. Retain hosted fallback and a documented emergency switch.
10. **Operations:** add alerts for offline/stuck runners, queue age, pod failures, capacity saturation, image age and GitHub App authentication failures.

## Acceptance evidence

The implementation issue is complete only when it links all of the following:

- controller ownership/version decision;
- GitHub App permission screenshot or exported permission inventory without secrets;
- External Secret manifest and successful reconciliation evidence;
- image digest, SBOM and vulnerability report;
- scale-set registration evidence;
- successful read-only smoke logs showing runner setup and ephemeral teardown;
- hosted-vs-self-hosted parity results for one Rust/Node job and one browser job;
- explicit disposition for service containers, Android KVM, macOS/iOS and Windows;
- rollback/emergency-disable steps;
- recurring Actions budget and usage-alert configuration retained as fallback.
