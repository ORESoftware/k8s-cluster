# Deploying the 3FA sync server

The canonical backend repository is vendored into ORES `k8s-cluster` as the
secondary git submodule `remote/deployments/3fa-backend`, matching the other
`remote/deployments/*` services. Code changes originate in this standalone
checkout; the cluster repo records only the reviewed upstream commit pointer.

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
- `deployment.yaml` – 2 replicas, non-root, read-only rootfs, liveness/readiness probes, OTLP/Kubernetes resource metadata, Loki-compatible JSON logging, and Prometheus discovery
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

## k8s-cluster submodule workflow

The submodule is already registered at `remote/deployments/3fa-backend` with
`git@github.com:3FA-app/3fa-backend.rs.git`. Verify the checkout without changing
it:

```bash
git -C ~/codes/ores/k8s-cluster submodule status remote/deployments/3fa-backend
git -C ~/codes/ores/k8s-cluster/remote/deployments/3fa-backend remote get-url origin
```

After a canonical commit passes CI and is pushed, update the secondary checkout
and commit only the gitlink change in the cluster repo:

```bash
cd ~/codes/ores/k8s-cluster
git submodule update --remote remote/deployments/3fa-backend
git add remote/deployments/3fa-backend
git commit -m "chore: bump 3fa-backend"
```

Do not develop inside the secondary checkout. Its working tree should remain
clean so a submodule update cannot hide or overwrite local work.

## Argo CD source boundary

`~/codes/ores/k8s-cluster/remote/argocd/apps/threefa-sync-server.application.yaml`
already registers the workload. Argo CD deliberately tracks the private
`3FA-app/3fa-backend.rs` repo directly because the cluster repo-server has
recursive git submodule checkout disabled. The k8s-cluster gitlink remains the
fleet inventory/build pin; Argo CD remains the live manifest reconciler. Keep
both pointers on the same reviewed upstream commit during a release.
