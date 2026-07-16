# Deploying the 3FA sync server

The backend is deployed to the ORES `k8s-cluster` as a **git submodule**, the same
way the other `remote/deployments/*` services are.

## Build the image

Self-contained build; context is the repo root:

```bash
docker build -f deploy/Dockerfile -t threefa-sync-server:dev .
```

Push to the registry the cluster pulls from (GHCR/ECR), then update
`image:` in `deploy/k8s/deployment.yaml`.

## Manifests

`deploy/k8s/` holds the full set, applied by ArgoCD:

- `namespace.yaml` – `threefa` namespace, PodSecurity `restricted`
- `deployment.yaml` – 2 replicas, non-root, read-only rootfs, liveness/readiness probes, OTLP resource metadata, and Prometheus discovery
- `service.yaml` – ClusterIP on 8080
- `networkpolicy.yaml` – default-deny; ingress from ingress-nginx/observability, egress to DNS, Postgres, and the namespace-scoped OTLP/HTTP collector only
- `externalsecret.yaml` – pulls the Postgres DSN into the `threefa-db` Secret
- `argocd-application.yaml` – the ArgoCD App

Validate without a cluster:

```bash
kubectl apply --dry-run=client -f deploy/k8s/
# or, against a live API server:
kubectl apply --dry-run=server -f deploy/k8s/
```

## Wiring into k8s-cluster as a submodule

This backend is added to `~/codes/ores/k8s-cluster` as a submodule at
`remote/deployments/3fa-backend`. The canonical GitHub repository is
`3FA-app/3fa-backend.rs`; the local source clone remains the day-to-day checkout:

```bash
cd ~/codes/ores/k8s-cluster
git submodule add git@github.com:3FA-app/3fa-backend.rs.git \
    remote/deployments/3fa-backend
git commit -m "add 3fa-backend deployment submodule"
```

### Verify the canonical GitHub URL

If an older clone still points at `ORESoftware`, switch it to the 3FA
organization:

```bash
cd ~/codes/ores/k8s-cluster
git submodule set-url remote/deployments/3fa-backend \
    git@github.com:3FA-app/3fa-backend.rs.git
git -C remote/deployments/3fa-backend remote set-url origin \
    git@github.com:3FA-app/3fa-backend.rs.git
git add .gitmodules && git commit -m "repoint 3fa-backend submodule to GitHub"
```

Then add an ArgoCD app entry under `remote/argocd/apps/` pointing at
`remote/deployments/3fa-backend/deploy/k8s` (or apply `argocd-application.yaml`).

> Note: a sync automation periodically commits/pushes ORES repos
> (`chore: sync local changes`); expect some churn around the submodule wiring.
