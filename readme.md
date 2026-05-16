# k8s-cluster

GitOps repository for the EC2 Kubernetes runtime synced by Argo CD.

The cluster apps live under `remote/argocd/`, service source lives under `remote/`, and the
worker/runtime manifests target the `dev` branch of `git@github.com:ORESoftware/k8s-cluster.git`.

The Node.js remote-dev worker is repo-configurable. It must receive `DD_REPO_URL` explicitly at
runtime or through generated thread manifests; it should not default to this repository or any other
Git repository.
