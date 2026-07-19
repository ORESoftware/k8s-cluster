# Sonus Auris deployment follow-ups

The intended end state is for `sonus-auris-monorepo` to be the canonical source revision for both
Sonus Auris application code and the deployment inputs consumed by this repository. As an
intermediate step, the Argo-managed console builds the console revision pinned by the checked-out
monorepo instead of cloning it with a personal token. The Kubernetes desired-state resources still
live in `k8s-cluster` until the remaining GitOps source migration is reviewed.

## GitOps source migration

- [ ] Add a GitHub App installation token (preferred) or repository-scoped deploy keys for private
  Sonus Auris builds that cannot use the node's read-only monorepo checkout. Do not reuse a personal
  classic token in Kubernetes.
- [ ] Move or generate the Sonus Auris backend and console deployment inputs from
  `sonus-auris-monorepo`, pin every component revision, and make the monorepo workflow update those
  pins through review.
- [ ] Point the Argo Sonus Auris application at the monorepo-owned path after a rendered-manifest
  comparison proves it is semantically equivalent to the current `dd-next-runtime` resources.
- [ ] Retire the duplicated `k8s-cluster` Sonus manifests only after the monorepo application is
  healthy and rollback to the previous Argo revision has been tested.

## Builder and registry

- [x] Separate ECR publication from application storage credentials. `dd-build-server` now reads a
  dedicated External Secret backed by an IAM identity restricted to the Sonus Flutter builder
  repository.
- [ ] Finish publishing `sonus-flutter-builder:3.44.2-c9a6c48423`, record its immutable ECR digest,
  review its registry scan, and replace tag references with that digest.
- [ ] Install and configure the Kubernetes ECR image credential provider during node bootstrap so
  new nodes can pull private builder images with the instance role. A locally cached image is not a
  durable deployment mechanism.
- [ ] Run the Android, Flutter web, Linux desktop, Playwright, and Puppeteer profiles on the cluster;
  keep iOS/macOS on macOS runners and Windows desktop on Windows runners.

## Fiducia locks and leases

- [ ] Provision a service-specific Fiducia API key for `dd-build-server` with `locks:write` and the
  least idempotency claim/complete/abandon scope supported by Fiducia. Store it in the build-server
  AWS Secrets Manager object and sync it through External Secrets.
- [ ] Smoke-test lock acquisition, fencing-token persistence, release, and idempotency lease
  lifecycle from the labeled build-server pod.
- [ ] Change `BUILD_SERVER_COORDINATION_REQUIRED` to `true` only after those tests pass and alerts
  cover repeated coordination failures.
- [ ] Evaluate replacing the local Rust HTTP adapter with the official `fiducia-clients` Rust API
  once its supported crate/API contract is stable; preserve the current URL policy, deadlines,
  fencing-token checks, and fail-closed production option during migration.

## Release readiness

- [ ] Validate the public backend health/readiness/API routes and the Flutter console through the
  production gateway with DNS and TLS in place.
- [ ] Run authenticated Supabase and upload/download smoke tests from physical Android and iPhone
  devices before store submission.
- [ ] Complete the Apple, Google Play, Supabase legacy-key migration, and Cloudflare R2 tasks tracked
  in the Flutter repository's `docs/publishing-todos.md`.
