# Production GitOps state

This directory is the production delivery boundary for Fiducia. GitHub Actions
promotes an exact superproject pin set into self-contained manifests; Argo CD
pulls those manifests from `main` and reconciles them continuously.

- `release.json` is the release bill of materials: component commits, registry
  digests, the infrastructure commit, and hashes of every rendered manifest.
- `data-plane/` contains the rendered state for the Hetzner, Civo, and Vultr
  clusters. It includes no Kubernetes Secrets.
- `argocd/` bootstraps the restricted production AppProject and ApplicationSet.

The ORESoftware Kubernetes cluster is the web plane. Its existing Argo CD setup
hosts `fiducia-admin`, `fiducia-backend` (the customer app), and `fiducia-auth`.
It must not be labeled `fiducia.cloud/plane=data`; the production ApplicationSet
will therefore never fan the Raft data plane into that cluster.
