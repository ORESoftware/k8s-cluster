# Azure cluster bootstrap profile

This is the future Azure Kubernetes cluster's GitOps root. It creates the Azure Disk-backed
`dd-block` StorageClass, installs External Secrets Operator, and registers the manual multi-cloud
CockroachDB/NATS Applications. It does not invent an Azure Key Vault, tenant ID, managed identity,
private DNS zone, VNet peer, or credential.

Before applying this root, add a reviewed Azure Key Vault provider overlay that creates
`ClusterSecretStore/dd-cluster-secrets` using workload identity. Then meet every gate in
`docs/multicloud-cockroachdb-nats.md`. Until that work is complete, the prerequisites Application
must remain unsynced.

Bootstrap once ArgoCD and the Azure Disk CSI driver exist:

```sh
kubectl apply -k remote/argocd/clusters/azure
```

The `dd-root-azure` Application subsequently self-manages this entry point from `dev`. The six
data-plane child Applications remain manual after registration.
