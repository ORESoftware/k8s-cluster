# k8s-cluster integration

Add this repository as a submodule under the cluster repo:

```sh
cd ~/codes/ores/k8s-cluster
git submodule add https://github.com/ORESoftware/mip-solver-node.rs.git remote/deployments/mip-solver-node.rs
```

Then add an Argo CD Application pointing directly at this repository's k8s bundle:

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
    repoURL: https://github.com/ORESoftware/mip-solver-node.rs.git
    targetRevision: main
    path: k8s
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

The submodule remains useful for EC2/local verification and for pinning a deployable revision in `k8s-cluster`, but Argo CD should not render through the submodule path. Some repo-server configurations expose gitlinks without checking out nested submodules, which makes `remote/deployments/mip-solver-node.rs/k8s` look like a missing path.

At runtime the pod still clones `k8s-cluster` because the Cargo manifest expects generated crates under `remote/libs/...` and the DES library under `remote/submodules/discrete-event-system.rs`. The manifests then clone this solver repo into `remote/deployments/mip-solver-node.rs` from `MIP_SOLVER_NODE_GIT_URL`/`MIP_SOLVER_NODE_GIT_REF`, so the running code follows the solver repo rather than a stale cluster submodule pointer.

## Helper script

From this repository checkout, a writable shell can run:

```sh
./ops/install-into-k8s-cluster.sh ~/codes/ores/k8s-cluster
```

The script adds or updates the submodule, copies the Argo CD Application manifest into `remote/argocd/apps/`, and validates the checked-out `k8s/` bundle with `kubectl kustomize`.

## GPU worker overlay

The default `k8s/` bundle is CPU-safe and sets slave pods to
`MIP_SOLVER_GPU_MODE=auto`. A worker uses CUDA/cuBLAS only when an NVIDIA
device is visible and the CUDA runtime libraries are present; otherwise it
falls back to in-house CPU matrix preprocessing.

For a GPU-backed worker pool, point Argo CD at:

```yaml
path: k8s-gpu
```

That overlay requests one `nvidia.com/gpu` for each slave pod and sets
`MIP_SOLVER_GPU_MODE=require`, so missing CUDA support fails fast instead of
silently running as CPU-only. It expects the cluster to have the NVIDIA device
plugin/container runtime installed and an image/runtime that provides
`libcudart` and `libcublas`.
