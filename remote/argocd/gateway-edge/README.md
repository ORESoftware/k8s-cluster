# gateway-edge — cloud-agnostic edge for `dd-remote-gateway`

This is the **target structure** for keeping the gateway's network edge cloud-agnostic
across AWS, Hetzner and GCP from one Git repo/commit/branch. It is **scaffolding** —
not yet wired to any live ArgoCD `Application`. Today the working edge lives inline in
`remote/argocd/dd-next-runtime/` (Service is cloud-neutral; the Ingress is active on
Hetzner and inert on AWS). Adopt this structure when GCP comes online and you want each
cloud to supply its own host/issuer instead of carrying Hetzner's in the shared layer.

## Principle (base + overlays, no provider `if/else`)

```
base/      cloud-neutral: the gateway ClusterIP Service (the Deployment + nginx
           routing ConfigMap stay in dd-next-runtime — they are 100% cloud-neutral)
overlays/
  aws/     single node, no ingress controller: patch hostPort 80/443 onto the pod and
           self-terminate TLS (secret dd-remote-gateway-tls, renewed by certbot on the
           host). No Ingress.
  hetzner/ LB -> ingress-nginx -> gateway: Ingress (class nginx, cert-manager
           ClusterIssuer letsencrypt-prod), host hello.95-217-171-250.sslip.io.
  gcp/     identical to hetzner except the host is the GKE ingress LB's sslip name.
```

The only thing that differs per cloud is the **edge** (how traffic ingresses + how TLS
is obtained). Everything behind it — the gateway pod and its `/home`, `/auth`, `/jello`
routes — is shared and identical everywhere.

## How each cluster selects its overlay

The two existing clusters run **independent, self-managing ArgoCD** instances that each
read this same repo, and the per-app `Application` manifests in `remote/argocd/apps/`
are bootstrapped to every cluster identically — so a single `Application` cannot diverge
by cloud. To adopt overlays, introduce an **ApplicationSet with a cluster generator**
(`applicationset.yaml` here) and label each cluster's ArgoCD cluster secret with
`dd.dev/cloud: aws|hetzner|gcp`. The ApplicationSet then templates the gateway-edge
`Application`'s `path` to `overlays/{{cloud}}`. One commit; each cluster picks its edge.

Migration order (do with eyes on the clusters, not unattended):
1. Land this scaffold (done) + label each ArgoCD cluster secret with its cloud.
2. Apply `applicationset.yaml`; confirm it generates one gateway-edge app per cluster
   pointing at the right overlay.
3. Remove the inline `dd-remote-gateway.ingress.yaml` from `dd-next-runtime/` (the
   overlay now owns it). The Service can stay shared or move to `base/`.
