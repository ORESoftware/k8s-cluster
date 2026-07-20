# Argo CD production bootstrap

Apply `kustomization.yaml` once to the Argo CD hub after registering the three
provider clusters. The hub then owns continuous reconciliation; GitHub Actions
never receives a kubeconfig.

Each registered data-plane cluster Secret must have all four labels:

```yaml
fiducia.cloud/cluster: "true"
fiducia.cloud/environment: production
fiducia.cloud/plane: data
fiducia.cloud/provider: hetzner # or civo / vultr
```

Do not put `fiducia.cloud/plane=data` on the ORESoftware web-plane cluster. That
cluster continues to host only admin, customer/backend, and auth via its own
Argo CD configuration in `~/codes/ores/k8s-cluster`.

The AppProject deliberately excludes `Secret`. Provision registry pull secrets,
TLS material, database URLs, Supabase credentials, and internal trust material
through the cluster secret-management plane before enabling auto-sync.
