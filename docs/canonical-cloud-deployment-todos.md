# canonical.plus monorepo deployment TODOs

## Decision

`canonical-cloud/canonical-monorepo` is the only deployable source of truth for
canonical.plus. A committed and tested monorepo SHA defines one release. Its
release workflow builds and attests the web, standalone API, and
session-revoker images from that exact checkout, and this repository promotes
only their immutable `sha256:` digests. Individual application repositories
and the `canonical-cloud/canonical.cloud` umbrella repository must not deploy
independently.

The Argo overlay follows that boundary. It never clones or builds source in the
cluster, and GitHub Actions never receives a kubeconfig. Argo CD is the only
runtime writer. Cloudflare supplies public routing and defense in depth, while
the Rust origins independently verify every Shared Auth credential.

## Temporary state

- The monorepo release workflow is being extended to report three image
  digests. A human currently runs
  `remote/argocd/canonical-cloud/promote-release.mjs`, reviews the resulting
  change, and opens the promotion commit or pull request.
- `remote/deployments/canonical-cloud` remains a secondary submodule pointing
  at the umbrella `canonical.cloud` repository. The canonical.plus Argo
  overlay does not consume it; it is legacy operational context only.
- The legacy `dd-canonical-cloud` workload remains available until the
  digest-based deployment passes the activation and cutover gates.
- The Canonical Shared Auth and Cloudflare edge changes live in separate
  repositories and must be promoted before the application hosts are enabled.
- `canonical-interfaces` already publishes a quote schema, but the current Rust
  quote implementation has not yet been generated from or reconciled with that
  contract. Do not call the cross-language client surface stable until this is
  resolved.

## Follow-ups

- [ ] **P0 — Publish the API artifact.** Merge the standalone API Docker target,
  pin the monorepo submodule to that exact reviewed commit, then publish and
  attest all three images from one successful monorepo release.
- [ ] **P0 — Automate reviewed digest promotion.** Install a narrow GitHub App
  or machine identity that can read monorepo release metadata and open pull
  requests against `k8s-cluster@dev`, but cannot apply to the cluster. Have a
  successful `canonical-monorepo` release open a digest-only promotion PR.
- [ ] **P0 — Verify release provenance.** Before opening the promotion PR,
  verify that all three digests were built from the requested monorepo SHA and
  that their attestations satisfy repository policy. Reject mutable tags.
- [ ] **P0 — Activate first-party identity and edge routing.** Deploy the
  Canonical Shared Auth realm and `/shared-auth/*` origin route, then attach
  exact Cloudflare Worker routes and proxied DNS for `app.canonical.plus` and
  `api.canonical.plus`. Never make Cloudflare the sole authorization layer.
- [ ] **P0 — Reconcile quote contracts.** Make `canonical-interfaces` the
  versioned source for quote request, response, status, and WebSocket event
  schemas; regenerate Rust and client bindings; and add compatibility tests.
- [ ] **P1 — Retire the umbrella checkout.** Remove
  `remote/deployments/canonical-cloud`, or repoint the secondary checkout to
  `canonical-monorepo` if an operator source checkout still has a documented
  use. Never make a pod clone or build that checkout.
- [ ] **P1 — Exercise promotion and rollback end to end.** Test the generated
  digest-only PR, Argo reconciliation, health/readiness, magic-link login,
  owner isolation, REST recovery, CSRF rejection, revocation, Gemini failure
  handling, WebSocket reconnects, and a Git-revert rollback.
- [ ] **P1 — Complete the legacy cutover.** Remove `dd-canonical-cloud` only in
  a separate reviewed change after production validation and an agreed
  rollback window.

## Invariants

- No kubeconfig or direct `kubectl apply` in canonical.plus GitHub Actions.
- No source clone, package install, image build, or database migration in a
  runtime pod.
- No automatic schema migration in Argo, CI, an init container, or server
  startup.
- No Supabase service-role key or privileged migration URL in any long-lived
  runtime.
- `GEMINI_API_KEY` is available only to the API workload.
- Browser cookies are host-only to `app.canonical.plus`; API clients use Shared
  Auth bearer tokens.
- Every deployed image is an immutable digest produced by the same tested
  `canonical-monorepo` SHA.
