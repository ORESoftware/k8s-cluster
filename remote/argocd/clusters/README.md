# Cluster GitOps Entry Points

These Kustomize overlays are the cloud-specific ArgoCD bootstrap entry points for the same
`dev` branch:

- `aws/` for the EC2 kubeadm cluster.
- `gcp/` for a GKE or kubeadm-on-GCP cluster.
- `hetzner/` for the Hetzner kubeadm cluster.

## Self-management (root app-of-apps)

Each overlay contains a self-referencing root Application — `dd-root-aws`, `dd-root-gcp`,
`dd-root-hetzner` — that syncs its own `remote/argocd/clusters/<cloud>` path (`automated`,
`prune`, `selfHeal`). This makes the entry point a true GitOps root rather than a one-shot
apply: after bootstrap, edits to a cluster's Application set on `dev` reconcile automatically,
out-of-band changes to the Application CRs heal back to Git, and each root surfaces in
`argocd_app_info` for health and sync alerting.

Bootstrap is unchanged: `kubectl apply -k remote/argocd/clusters/<cloud>` renders the root
Application with every current child Application, including newer GHA continuity applications.
The root then adopts and manages the complete overlay, including itself. It carries no cascade
finalizer, so pruning a removed child deletes only its Application CR and orphans workloads
rather than cascade-deleting them. See `docs/app-deploy-contract.md` for why the root source
must stay inside `k8s-cluster` instead of a submodule gitlink.

Keep application manifests cloud-neutral. The core ArgoCD Applications point at shared app paths
such as `remote/argocd/dd-next-runtime`, `remote/argocd/messaging`, and
`remote/argocd/observability`; cloud differences live in provider overlays.

The shared secret contract is `dd-cluster-secrets`. Each cluster profile creates one provider
Application for the matching store, plus one common Application for the shared ExternalSecrets:

- AWS: `remote/argocd/secrets/providers/aws` and `remote/argocd/secrets/common`
- GCP: `remote/argocd/secrets/providers/gcp` and `remote/argocd/secrets/common`
- Hetzner: `remote/argocd/secrets/providers/hetzner` and `remote/argocd/secrets/common`

The shared block-storage contract is `dd-block`. Provider overlays map it to EBS CSI, GCE PD CSI,
or Hetzner CSI without changing app manifests.
