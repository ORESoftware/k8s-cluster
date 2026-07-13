# workflows

GitHub Actions for the superproject.

- `ci.yml` — public PR contract CI initializes only the public interface/sync
  gitlinks used by the tests, while still validating all 26 entries as gitlinks.
  Trusted `main` and manual runs add a full recursive fleet audit and require the
  read-only `FIDUCIA_SUBMODULE_TOKEN`; missing private access fails that job.
- `deploy.yml` — the ONLY path to production, and manual only
  (`workflow_dispatch`, never push-triggered). It validates the rendered infra
  topology and every cluster kustomization from `apps/fiducia-infra`, then does a
  credential-gated kubectl rollout. Missing submodule or kubeconfig credentials
  fail closed; use `ci.yml` for validation-only runs. See `docs/deploy.md`.
