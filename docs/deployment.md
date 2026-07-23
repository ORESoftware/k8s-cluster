# Deployment handoff

No command in this document should be interpreted as approval to change the cluster. Validate first and apply only after an explicit deployment decision.

## Prerequisites

1. GitHub Actions in all three Rust repositories must finish and publish the `main` images to GHCR.
2. Argo CD must already have read access to the private `drone-mngr` repositories.
3. The cluster's `dd-cluster-secrets` store must expose `dd/remote-dev/drone-mngr-runtime` with the database, shared-auth, Supabase, public-origin, and session-encryption environment values. Fiducia/Vault may own or hydrate this platform record; the pods receive only the projected variables.
4. The Postgres schemas in the control and web applications' `db/schema.sql` files must be applied through the cluster's normal migration process.
5. Replace the Cloudflare origin placeholders and supply Cloudflare credentials outside Git before any OpenTofu or Wrangler operation.

## Validate

From the monorepo root:

```sh
nix develop
scripts/check
kubectl kustomize apps/drone-mngr-infra/argocd >/dev/null
kubectl kustomize apps/drone-mngr-ctrl-server.rs/deploy/k8s >/dev/null
kubectl kustomize apps/drone-mngr-mcp-server.rs/deploy/k8s >/dev/null
kubectl kustomize apps/drone-mngr-web-server.rs/deploy/k8s >/dev/null
```

The one-time Argo CD entrypoint is `apps/drone-mngr-infra/bootstrap/drone-mngr-root.application.yaml`. It deliberately remains a reviewed bootstrap artifact rather than self-applying automation.

For production releases, replace mutable `:main` image references with the immutable `sha-<commit>` tags emitted by each application's workflow, commit those manifest changes, and let Argo CD reconcile the reviewed revision.
