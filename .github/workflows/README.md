# workflows

GitHub Actions for the superproject.

- `ci.yml` — public PR contract CI initializes only the public interface/sync
  gitlinks used by the tests, while still validating all 26 entries as gitlinks.
  Trusted `main` and manual runs add a full recursive fleet audit and require the
  read-only `FIDUCIA_SUBMODULE_TOKEN`; missing private access fails that job.
- `deploy.yml` — the ONLY path to production, and manual only
  (`workflow_dispatch`, never push-triggered). It validates the rendered infra
  topology and every cluster kustomization from `apps/fiducia-infra`, verifies
  and pins core GHCR images to the reviewed component gitlink SHAs, then applies
  each overlay to its explicitly named kubeconfig context and waits for the
  rollouts. Missing images, submodules, contexts, or credentials fail closed;
  use component CI for validation-only runs. See `docs/deploy.md`.

Component repositories may test and publish immutable artifacts, but they do
not hold Kubernetes or Cloudflare deployment credentials. The marketing Pages
workflow is the platform-bound exception described in `docs/deploy.md`.

## Security baseline

Every executable workflow uses explicit least-privilege permissions, immutable
third-party action or container references, non-persisted checkout credentials,
concurrency control, and a job timeout. The main CI workflow validates this
directory with the digest-pinned actionlint container. Environment mutation is
forbidden unless this README documents a repository-specific platform exception.
