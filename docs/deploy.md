# Deployment

Two environments, two owners. The split keeps everyday iteration fast while
making production a single, deliberate, auditable action.

## TEST — owned by each app repo

Each app repo's own CI is responsible for the **test** environment on merge to
`main`:

- CI publishes the release image (e.g. `ghcr.io/fiducia-cloud/<app>`) and runs
  its **secret-gated** test-env rollout.
- The rollout is a **no-op without credentials**: if the repo has no test
  cluster/registry deploy secret, the step prints a notice and the job stays
  green. A fork or a freshly created repo never deploys anything by accident.
- Test deploys are per-repo and fast — they do **not** wait on the superproject
  and do **not** touch production.

## PROD — owned by the monorepo, manually

`fiducia-monorepo` is the **only** path to production. There is no push-triggered
prod deploy anywhere.

- Production ships **only** via `.github/workflows/deploy.yml`, and only when an
  operator dispatches it (`workflow_dispatch`) against a reviewed submodule pin
  set. The superproject pins are the deployable state: a component change reaches
  prod only after its repo is pushed **and** its pin here is updated (see
  `docs/repo-boundaries.md`).
- The workflow first **validates** the rendered state from `apps/fiducia-infra`
  (`node tools/render.mjs --check`, then `kubectl kustomize` for every
  `clusters/*/`) before any apply.
- The rollout step is **credential-gated and never automatic**: with an ArgoCD
  token or `KUBE_CONFIG_PROD` it performs the real `argocd app sync` /
  `kubectl apply`; with none it prints
  `no prod credentials configured — validation-only` and succeeds without
  touching prod. Required prod secrets are listed at the top of `deploy.yml`.
- Bind the `prod` GitHub Environment to **required reviewers** for a human
  approval gate on top of the manual dispatch.

## Why prod lives here and not in the app repos

Production is a property of the **whole fleet at a coherent set of pins**, not of
any single component. Centralizing it in the superproject means one reviewed pin
set, one manual trigger, one place to audit — while app repos keep shipping to
test independently.
