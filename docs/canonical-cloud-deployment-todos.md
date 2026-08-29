# canonical.cloud monorepo deployment TODOs

## Decision

`canonical-cloud/canonical-monorepo` is the only deployable source of truth for
canonical.cloud. A committed and tested monorepo SHA defines one release. Its
release workflow builds and attests the web and session-revoker images from
that exact checkout, and this repository promotes only their immutable
`sha256:` digests. Individual application repositories and the
`canonical-cloud/canonical.cloud` umbrella repository must not deploy
independently.

The current Argo overlay follows that boundary. It never clones or builds
source in the cluster, and GitHub Actions never receives a kubeconfig. Argo CD
is the only runtime writer.

## Temporary state

- The monorepo release workflow reports the two image digests, but a human
  currently runs `remote/argocd/canonical-cloud/promote-release.mjs`, reviews
  the resulting change, and opens the promotion commit or pull request.
- `remote/deployments/canonical-cloud` remains a secondary submodule pointing
  at the umbrella `canonical.cloud` repository. The canonical.cloud Argo
  overlay does not consume it; it is legacy operational context only.
- The legacy `dd-canonical-cloud` workload remains available until the
  digest-based deployment passes the activation and cutover gates.

## Follow-ups

- [ ] **P0 — Automate reviewed digest promotion.** Install a narrow GitHub App
  or machine identity that can read monorepo release metadata and open pull
  requests against `k8s-cluster@dev`, but cannot apply to the cluster. Have a
  successful `canonical-monorepo` release open a digest-only promotion PR.
- [ ] **P0 — Verify release provenance.** Before opening the promotion PR,
  verify that both digests were built from the requested monorepo SHA and that
  their attestations satisfy the repository policy. Reject mutable tags.
- [ ] **P1 — Retire the umbrella checkout.** Remove
  `remote/deployments/canonical-cloud`, or repoint the secondary checkout to
  `canonical-monorepo` if an operator source checkout still has a documented
  use. Never make a pod clone or build that checkout.
- [ ] **P1 — Exercise promotion and rollback end to end.** Test the generated
  digest-only PR, Argo reconciliation, health/readiness, REST authentication,
  HTMX responses, WebSocket reconnects, and a Git-revert rollback.
- [ ] **P1 — Complete the legacy cutover.** Remove `dd-canonical-cloud` only in
  a separate reviewed change after production validation and an agreed
  rollback window.

## Invariants

- No kubeconfig or direct `kubectl apply` in canonical.cloud GitHub Actions.
- No source clone, package install, image build, or database migration in a
  runtime pod.
- No automatic schema migration in Argo, CI, an init container, or server
  startup.
- No Supabase service-role key in either long-lived runtime.
- Every deployed image is an immutable digest produced by the same tested
  `canonical-monorepo` SHA.
