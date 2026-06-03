# k8s-cluster integration

Add this repository as a submodule under the cluster repo:

```sh
cd ~/codes/ores/k8s-cluster
git submodule add https://github.com/ORESoftware/mip-solver-node.rs.git remote/deployments/mip-solver-node.rs
```

Then add an Argo CD Application pointing at the submodule's k8s bundle:

`remote/argocd/apps/dd-in-house-mip-solver-node.application.yaml`

with:

```yaml
apiVersion: argoproj.io/v1alpha1
kind: Application
metadata:
  name: dd-in-house-mip-solver-node
  namespace: argocd
spec:
  project: default
  source:
    repoURL: git@github.com:ORESoftware/k8s-cluster.git
    targetRevision: dev
    path: remote/deployments/mip-solver-node.rs/k8s
  destination:
    server: https://kubernetes.default.svc
    namespace: ai-ml
  syncPolicy:
    automated:
      prune: true
      selfHeal: true
    syncOptions:
      - CreateNamespace=true
```

Argo CD supports git submodules automatically unless `ARGOCD_GIT_MODULES_ENABLED=false` is set on repo-server.

## Helper script

From this repository checkout, a writable shell can run:

```sh
./ops/install-into-k8s-cluster.sh ~/codes/ores/k8s-cluster
```

The script adds or updates the submodule, copies the Argo CD Application manifest into `remote/argocd/apps/`, and validates the submodule's `k8s/` bundle with `kubectl kustomize`.
