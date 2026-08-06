# VPN-only Argo CD mobile prerequisites

This directory activates the certificate and least-privilege local account used by the private
phone endpoint implemented in the `dd-vpn` pod. It is intentionally **not** referenced by
`remote/argocd/vpn/kustomization.yaml`. The live VPN can roll out its DNS and proxy sidecars without
making cert-manager CRDs, a Cloudflare credential, or cross-namespace Argo CD configuration an
unreviewed merge-time dependency.

The endpoint is:

```text
https://argocd-vpn.fiducia.cloud:8443
```

CoreDNS inside the WireGuard pod resolves that name to `10.8.0.1`. There is no public A/AAAA record,
Service, NodePort, Ingress, or host port for `8443`. A publicly trusted certificate is issued with a
Cloudflare DNS-01 TXT challenge; the hostname remains reachable only through WireGuard.

## Preconditions

1. `dd-vpn` is healthy and the phone can establish a WireGuard tunnel.
2. cert-manager and its CRDs are installed and healthy on the target cluster. The repository's
   cert-manager Applications are also opt-in; follow `remote/argocd/cert-manager/README.md` rather
   than applying them blindly on AWS or taking over imperatively managed objects without watching
   the first sync.
3. External Secrets Operator and `ClusterSecretStore/dd-cluster-secrets` are healthy.
4. AWS Secrets Manager key `dd/remote-dev/cloudflare` contains
   `CLOUDFLARE_DNS_API_TOKEN`, scoped to `Zone:DNS:Edit` and `Zone:Zone:Read` for only the
   `fiducia.cloud` zone.

No GitHub, Cloudflare, Argo CD, or WireGuard secret belongs in Git.

## Activate deliberately

Apply the inert Application only after the preconditions are verified:

```bash
kubectl apply -f remote/argocd/apps/dd-argocd-mobile-prereqs.application.yaml
```

Then watch each boundary become ready:

```bash
kubectl -n cert-manager wait \
  --for=condition=Ready \
  externalsecret/argocd-mobile-cloudflare-dns-api-token \
  --timeout=5m

kubectl wait \
  --for=condition=Ready \
  clusterissuer/letsencrypt-prod-dns01-argocd-mobile \
  --timeout=5m

kubectl -n vpn wait \
  --for=condition=Ready \
  certificate/argocd-mobile-tls \
  --timeout=10m

kubectl -n vpn logs deployment/dd-vpn \
  -c argocd-mobile-proxy \
  --tail=100
```

The nginx sidecar waits harmlessly while the certificate is absent. It hashes the projected key and
certificate every 30 seconds and reloads nginx in place after renewal, without restarting WireGuard.

## Enroll the phone

`INIT_DNS=10.8.0.1` affects only a fresh wg-easy database. For an existing installation, update the
DNS setting in the private wg-easy UI and re-export the phone peer, or edit that peer's profile so
its `DNS` line is `10.8.0.1`.

Establish the local account password interactively; do not put it in a command argument, environment
file, manifest, issue, or PR:

```bash
argocd login argocd-vpn.fiducia.cloud:8443 \
  --username admin \
  --grpc-web

argocd account update-password \
  --account argocd-mobile \
  --grpc-web
```

Verify private DNS and trusted TLS from a WireGuard-connected operator machine:

```bash
nslookup argocd-vpn.fiducia.cloud 10.8.0.1
curl --fail --show-error --silent \
  https://argocd-vpn.fiducia.cloud:8443/healthz
```

In the Argo CD mobile app, add:

```text
Server:   https://argocd-vpn.fiducia.cloud:8443
Username: argocd-mobile
Password: <the interactively established password>
```

The account can inspect applications, resource trees, projects, clusters, repositories,
certificates, and logs. It cannot sync, update, delete, execute in pods, or invoke resource actions.
On Android, strict Private DNS can bypass a VPN-provided resolver; use `Automatic` while this tunnel
is active if the private hostname does not resolve.

## Optional project-scoped sync

Do not start with fleet-wide write access. After read-only enrollment is verified, add only the
specific Argo CD project that the phone is allowed to reconcile:

```text
p, argocd-mobile, applications, sync, <project>/*, allow
```

Never replace `<project>` with `*` merely for convenience. A sync is an operational write and can
prune resources when the target Application is configured to prune.

## Rollback and revocation

The fastest revocation is to remove the phone's WireGuard peer or set
`accounts.argocd-mobile.enabled: "false"` in `argocd-cm`. Deleting the opt-in Application does not
prune these prerequisites because `prune` is deliberately disabled; remove individual resources
only after confirming which controller owns them.

Reverting the `dd-vpn` sidecars also removes the VPN-local DNS listener. Before doing so, restore an
ordinary DNS server in wg-easy and refresh existing phone profiles, otherwise clients still pointing
at `10.8.0.1` will lose name resolution. Never delete cert-manager CRDs as part of this rollback.
