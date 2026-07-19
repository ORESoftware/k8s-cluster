# Benefactor deployment follow-ups

## Move the deployment source to the benefactor monorepo

Preferred end state: build and release the Benefactor backend and frontend from
one canonical benefactor monorepo, while keeping this cluster repository as the
GitOps source of truth for deployed image digests and Kubernetes configuration.

Current temporary state (2026-07-18):

- `benefactor-cc/backend.rs@main` validates and publishes the backend image.
- Its release workflow commits the immutable image digest to
  `remote/argocd/benefactor-backend-rs/kustomization.yaml` on this repository's
  `dev` branch.
- Argo CD reconciles that path. This is safe to operate until the monorepo cutover.
- No repository or local checkout named `benefactor-monorepo` currently exists
  in the `benefactor-cc` organization map, so the release source must not be
  switched to an assumed URL.

Cutover checklist:

- [ ] Identify or create the canonical monorepo and record its GitHub URL,
      default branch, owners, and local checkout in the shared repo map.
- [ ] Move the backend and frontend source with history and preserve their test,
      security, telemetry, and interface contracts.
- [ ] Add path-aware monorepo CI that runs Rust quality gates, frontend tests,
      dependency audits, and production builds only for affected applications.
- [ ] Publish the backend image from the monorepo. Prefer keeping
      `ghcr.io/benefactor-cc/backend.rs` stable during the first cutover; rename
      the package only in a separately reversible migration.
- [ ] Move the `K8S_GITOPS_DEPLOY_KEY` to the monorepo Actions secrets, update
      the OCI source label, and keep digest-only promotion into this repository.
- [ ] Point frontend production publishing at the monorepo workflow while
      retaining `benefactor-cc/benefactor-cc.github.io` as generated output.
- [ ] Prove a release and rollback through Argo, then disable the old standalone
      release workflows only after parity is verified.
- [ ] Update `AGENTS.md`, runbooks, ownership metadata, and rotation dates so no
      operator is directed back to the standalone repositories.
