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

## GPU worker overlay

The default `k8s/` bundle is CPU-safe and sets slave pods to
`MIP_SOLVER_GPU_MODE=auto`. A worker uses CUDA/cuBLAS only when an NVIDIA
device is visible and the CUDA runtime libraries are present; otherwise it
falls back to in-house CPU matrix preprocessing.

For a GPU-backed worker pool, point Argo CD at:

```yaml
path: remote/deployments/mip-solver-node.rs/k8s-gpu
```

That overlay requests one `nvidia.com/gpu` for each slave pod and sets
`MIP_SOLVER_GPU_MODE=require`, so missing CUDA support fails fast instead of
silently running as CPU-only. It expects the cluster to have the NVIDIA device
plugin/container runtime installed and an image/runtime that provides
`libcudart` and `libcublas`.
