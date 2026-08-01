# Cluster GitOps Entry Points

These Kustomize overlays are the cloud-specific ArgoCD bootstrap entry points for the same
`dev` branch:

- `aws/` for the EC2 kubeadm cluster.
- `gcp/` for a GKE or kubeadm-on-GCP cluster.
- `hetzner/` for the Hetzner kubeadm cluster.

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
