# workflows

GitHub Actions for the superproject.

- `ci.yml` validates the superproject contracts and committed GitOps release on
  pull requests and `main`. Explicit manual runs additionally audit the complete
  recursive fleet with the read-only `FIDUCIA_SUBMODULE_TOKEN`; the production
  promotion always performs that same fail-closed recursive audit.
- `deploy.yml` is a manual, `prod`-environment-gated promotion. It resolves the
  reviewed component gitlinks to GHCR digests, renders the three provider
  overlays, validates them, and commits only desired state to `main` for Argo CD
  to reconcile.

No workflow contains a kubeconfig, cloud credential, direct cluster mutation,
or Argo API token. Component repositories test and publish immutable artifacts;
they do not deploy environments. See `docs/deploy.md`.

## Security baseline

Every executable workflow uses explicit least-privilege permissions, immutable
third-party action or container references, non-persisted checkout credentials,
concurrency control, and a job timeout. The CI workflow validates both workflow
files with the digest-pinned actionlint container.
