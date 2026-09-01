# Cluster GitOps Entry Points

These Kustomize overlays are the cloud-specific ArgoCD bootstrap entry points for the same
`dev` branch:

- `aws/` for the EC2 kubeadm cluster.
- `gcp/` for a GKE or kubeadm-on-GCP cluster.
- `azure/` for the future AKS cluster bootstrap profile.
- `hetzner/` for the Hetzner kubeadm cluster.

## Self-management (root app-of-apps)

Each overlay contains a self-referencing root Application — `dd-root-aws`, `dd-root-gcp`,
`dd-root-azure`, `dd-root-hetzner` — that syncs its own `remote/argocd/clusters/<cloud>` path (`automated`,
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

Azure deliberately has no secret-store overlay yet: tenant, Key Vault, and workload identity
values must be provisioned and reviewed rather than committed as placeholders. Its multi-cloud
data-plane prerequisites stay manual until `ClusterSecretStore/dd-cluster-secrets` exists.

The shared block-storage contract is `dd-block`. Provider overlays map it to EBS CSI, GCE PD CSI,
Azure Disk CSI, or Hetzner CSI without changing app manifests.

AWS, GCP, and Azure also register the manual Applications under
`remote/argocd/multicloud-data-plane`. Read `docs/multicloud-cockroachdb-nats.md`; registration is
not authorization to sync stateful quorums.
