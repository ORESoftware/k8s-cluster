# Deployment

One deployment owner, many test owners. Component repositories validate their
code and publish immutable artifacts; the superproject is the only repository
allowed to mutate a runtime environment.

## Component repositories — tests and artifacts only

Each app repo's own CI is responsible for validating changes on pull requests
and `main`:

- Test workflows use locked dependency graphs, immutable sibling revisions,
  least-privilege tokens, bounded jobs, and non-persisted checkout credentials.
- Container-producing repositories publish a commit-SHA tag with provenance and
  an SBOM. They do not publish `latest` and do not receive kubeconfig or
  Cloudflare deployment credentials.
- The marketing site's GitHub Pages workflow is the only exception because the
  Pages OIDC token and environment are bound by GitHub to that repository. Its
  write permissions exist only on the deploy job.

## Runtime deployment — owned by the monorepo, manually

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
- Core workload images are derived from the reviewed component gitlinks. The
  workflow verifies that each commit-SHA image exists in GHCR, rewrites the
  runner's manifest copy to those exact SHAs, and rejects mutable core tags.
- Each cluster overlay is applied only to a kubeconfig context with the same
  explicit cluster name (`hetzner`, `vultr`, `civo`). A missing context fails
  the deployment instead of falling back to the ambient/current context. Every
  node, brain, and load-balancer rollout must then complete.
- The rollout step is **credential-gated and never automatic**. It requires
  `KUBE_CONFIG_PROD` plus a read-only fine-grained
  `FIDUCIA_SUBMODULE_TOKEN` that can clone every private app repo. Missing
  credentials fail the manual workflow. Public PR CI is the contract-only
  validation path; trusted `main` CI additionally runs the recursive fleet
  audit with the same read-only token. Required secrets are listed at the top
  of `deploy.yml`.
- Bind the `prod` GitHub Environment to **required reviewers** for a human
  approval gate on top of the manual dispatch.

## Why deployment lives here and not in the app repos

Deployment is a property of the **whole fleet at a coherent set of pins**, not
of any single component. Centralizing it in the superproject means one reviewed
pin set, one manual trigger, one place to audit, and no race between independent
component rollouts.
