# Sonus Auris CI runner image

This image is the proposed non-privileged Linux worker for the ARC scale set
labeled `sonus-ci`. It extends GitHub's official Actions runner image and adds
native compiler, desktop, and browser libraries used by Sonus Auris Rust,
Node/browser, Flutter web, and Flutter Linux jobs. Package-managed browser
binaries remain selected and installed by each repository's lockfile/workflow.
native libraries used by Sonus Auris Rust, browser, Flutter web, and Flutter
Linux jobs.

It intentionally does **not** provide Docker-in-Docker, a host Docker socket,
Kubernetes credentials, Android KVM, Apple toolchains, signing credentials, or
production cloud credentials.

## Build and pin

Build from the repository root so the source path is explicit:

```sh
docker build \
  --pull \
  --build-arg RUNNER_VERSION=2.334.0 \
  -f remote/deployments/sonus-auris-ci-runner/Dockerfile \
  -t ghcr.io/oresoftware/sonus-auris-ci-runner:2.334.0-1 \
  .
```

Before publishing:

1. inspect the upstream runner release and enforced minimum version;
2. scan the image and fail on actionable high/critical findings;
3. generate an SBOM and provenance attestation;
4. push an immutable version tag;
5. obtain the registry digest;
6. replace `REPLACE_IMAGE_DIGEST` in the runner-set template with that digest.

Do not deploy a mutable tag such as `latest`.

## Validation contract

A candidate image must prove all of the following in an isolated test namespace:

- the runner registers and accepts exactly one ephemeral job;
- the pod/workspace is destroyed after the job;
- `actions/setup-node`, `dtolnay/rust-toolchain`, `dart-lang/setup-dart`,
  `subosito/flutter-action`, and `actions/setup-java` can install their pinned
  toolchains without root access;
- Rust formatting, clippy, tests, docs, and packaging work;
- Node lockfile installation and the repository-selected
  Puppeteer/Playwright/Selenium browser binaries and smokes work;
- Node lockfile installation and Chromium-based Puppeteer/Playwright/Selenium
  smokes work;
- Dart formatting, analysis, tests, and package dry-runs work;
- Flutter analysis/tests, production web builds, and Linux desktop compilation
  work;
- no service-account token, cluster credential, cloud credential, or GitHub App
  private key is present inside the job container;
- workspace, tool cache, and `/tmp` stay within their configured emptyDir limits.

## Deliberate exclusions

The following require separate lanes and threat models:

- workflows declaring GitHub Actions `services:` or building OCI images require
  a Docker-in-Docker runner set with a privileged sidecar;
- Android emulator jobs require KVM-capable nodes and reviewed device mounts;
- iOS/macOS compile, signing, notarization, and App Store upload require Apple
  hardware and cannot run in this Linux Kubernetes runner.

A positive hosted Actions budget remains required for the complete Sonus Auris
mobile release matrix even after this Linux lane is operational.
