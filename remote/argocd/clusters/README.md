# Cluster GitOps Entry Points

These Kustomize overlays are the cloud-specific ArgoCD bootstrap entry points for the same
`dev` branch:

- `aws/` for the EC2 kubeadm cluster.
- `gcp/` for a GKE or kubeadm-on-GCP cluster.
- `hetzner/` for the Hetzner kubeadm cluster.

## Self-management (root app-of-apps)

Each overlay contains a self-referencing root Application — `dd-root-aws`, `dd-root-gcp`,
`dd-root-hetzner` — that syncs its own `remote/argocd/clusters/<cloud>` path (`automated`,
`prune`, `selfHeal`). This is what makes the entry point a true GitOps root rather than a
one-shot apply: after bootstrap, edits to a cluster's Application set on `dev` reconcile
automatically, out-of-band changes to the Application CRs are healed back to git, and each
root surfaces in `argocd_app_info` for health/sync alerting.

Bootstrap is unchanged — `kubectl apply -k remote/argocd/clusters/<cloud>` renders the root
Application along with everything else, and it then adopts and manages the whole overlay
(including itself). The root carries no cascade finalizer, so pruning a removed child deletes
only its Application CR (its workloads are orphaned, never cascade-deleted). See
`docs/app-deploy-contract.md` for why the root's self-source path must stay inside k8s-cluster
(not a submodule gitlink) to render.

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
