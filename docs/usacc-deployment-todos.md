# USA-ACC deployment follow-ups

## Make the USA-ACC monorepo the canonical deployment source

Preferred end state: build, validate, and promote the USA-ACC backend from
`usa-acc/usacc-monorepo`, while keeping this cluster repository as the GitOps
source of truth for Kubernetes configuration and immutable deployed revisions.

Current temporary state (2026-07-18):

- `usa-acc/rest-api-backend.rs@main` owns the integrated backend CI and GitOps
  promotion job.
- A successful backend build updates the backend gitlink and immutable source
  revision in this repository's `dev` branch.
- Argo CD is the sole reconciler for the resulting cluster deployment.
- This standalone path remains supported until the monorepo provides equivalent
  validation, promotion, and rollback behavior.

Cutover checklist:

- [ ] Define the backend package and service boundaries in
      `usa-acc/usacc-monorepo`, preserving source history and API contracts.
- [ ] Move the integrated backend CI gates into path-aware monorepo jobs,
      including formatting, strict Clippy, unit and contract tests, and the
      current Kubernetes dependency assembly checks.
- [ ] Update the cluster gitlink and manifests to reference the monorepo source
      or its rendered GitOps output, always pinned to an immutable revision.
- [ ] Map the current Rust path dependencies and lockfile behavior into the
      monorepo workspace without weakening reproducibility.
- [ ] Run the monorepo fleet and contract tests plus all existing backend tests
      before the first promotion.
- [ ] Move the scoped GitOps deploy keys and repository secrets to the monorepo;
      remove the standalone promotion job only after a verified deployment.
- [ ] Prove Argo sync, health, rollout, and rollback from the monorepo, then
      update ownership and operator documentation.

