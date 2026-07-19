# Sonus Flutter builder image

Cluster-owned Android, web, and Linux Flutter toolchain used by fixed build
profiles and the Argo console build init container.

The image starts from the digest-pinned Cirrus Android SDK 36 base, installs the
Linux desktop toolchain, and fetches the exact Flutter framework revision
`c9a6c48423` (Flutter 3.44.2 / Dart 3.12.2). It is published to the immutable,
scan-on-push ECR repository `sonus-flutter-builder`; Kubernetes workloads should
pin the resulting digest after the bootstrap build succeeds.

The checkout is intentionally shallow, so the Dockerfile creates the reviewed
`3.44.2` release tag at the verified full revision before Flutter first writes
its version cache. Omitting that tag makes the tool report `0.0.0-unknown` and
causes otherwise-compatible packages to fail their Flutter SDK constraint.

The `dd-build-server` builds it from repository context `remote/images/sonus-flutter-builder`.
Changing Flutter or the Android base requires a reviewed Dockerfile change, a
new immutable tag, a successful ECR scan, and an Argo digest update.
