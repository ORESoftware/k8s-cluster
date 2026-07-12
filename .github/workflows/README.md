# workflows

GitHub Actions for the superproject.

- `ci.yml` — runs on push/PR. Checks out every submodule (`recursive`, rewriting
  the SSH submodule URLs to HTTPS so the default token can fetch the public app
  repos) and runs the `tests/*.test.mjs` contract tests that guard submodule
  wiring and the cross-repo sync contract.
- `deploy.yml` — the ONLY path to production, and manual only
  (`workflow_dispatch`, never push-triggered). It validates the rendered infra
  topology and every cluster kustomization from `apps/fiducia-infra`, then does a
  credential-gated ArgoCD/kubectl rollout. With no prod secrets it runs
  validation-only and exits green. See `docs/deploy.md`.
