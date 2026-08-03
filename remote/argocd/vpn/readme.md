# `remote/argocd/vpn`

GitOps manifests for `dd-vpn`, a WireGuard VPN endpoint managed by
[`wg-easy`](https://wg-easy.github.io/wg-easy/latest/getting-started/), plus `dd-bastion`, a
Rust access broker/jump-host service reachable through that VPN. Together they create a small
cluster-private VPN address space (`10.8.0.0/24`), an admin UI for creating WireGuard clients, an
authenticated place to retrieve cluster access profiles, and a VPN-only Argo CD endpoint suitable
for a phone dashboard.

The app uses `ghcr.io/wg-easy/wg-easy:15`; the wg-easy docs recommend pinning the major `15` tag
and avoiding `latest`, because `latest` still points at v14.

## What gets deployed

- Namespace: `vpn`
- Deployment: `dd-vpn`
- Deployment: `dd-bastion`
- Public VPN listener: UDP `51820` on the EC2 node via Kubernetes `hostPort`
- Admin UI: `dd-vpn-ui.vpn.svc.cluster.local:51821`, ClusterIP only
- Bastion/access broker: `dd-bastion.vpn.svc.cluster.local:8111`, ClusterIP only
- VPN DNS: `10.8.0.1:53`, implemented by a CoreDNS sidecar
- Phone Argo CD endpoint: `https://argocd-vpn.fiducia.cloud:8443`
- Persistent config: PVC `dd-vpn-config`, mounted at `/etc/wireguard`
- Secret source: AWS Secrets Manager key `dd/remote-dev/vpn-secrets`
- Bastion auth source: AWS Secrets Manager key `dd/remote-dev/agent-secrets`, synced into
  `dd-bastion-secrets`

The deployment uses a short privileged init container to set the network namespace sysctls that
WireGuard needs and prepare a private writable tmpfs for the non-root TLS proxy. The main wg-easy
container gets Linux networking capabilities (`NET_ADMIN`, `SYS_MODULE`) and a read-only mount of
`/lib/modules` so WireGuard can use the host kernel module. It runs as a single replica with
`Recreate` rollout strategy so only one pod ever owns the host UDP port and WireGuard state.

The CoreDNS and nginx sidecars share the pod network namespace with WireGuard. CoreDNS gives
`argocd-vpn.fiducia.cloud` the private answer `10.8.0.1` and forwards all other queries to kube-dns.
nginx listens on `8443`, accepts only `10.8.0.0/24`, terminates a public-trust certificate, and
proxies to the cluster-local `argocd-server`. There is deliberately no Service, NodePort, Ingress,
or hostPort for the phone endpoint.

`dd-bastion` is not a broad public SSH server. It is an authenticated Rust HTTP service that
operators reach either directly through WireGuard or indirectly through the public gateway's
`/bastion/...` paths:

- `GET /healthz` - unauthenticated health check.
- `GET /profile` or `/config` - VPN endpoint, DNS, service CIDR, pod CIDR, and cluster API info.
- `GET /kubeconfig` - read-only kubeconfig using the `dd-bastion` service account token.
- `GET /runtime/deployments` - live managed Deployment, Pod, and container inventory.

Direct bastion routes accept `X-Bastion-Auth`, `X-Server-Auth`, `Auth`, or
`Authorization: Bearer ...` with `SERVER_AUTH_SECRET`. Public gateway-proxied `/bastion/...`
routes first require the gateway `Auth` header or `dd_auth` cookie value from
`dd-remote-auth-secrets` / `DD_AUTH_COOKIE_VALUE`; after that the gateway injects the internal
`X-Server-Auth` value for bastion. The generated kubeconfig is bound to
`ClusterRole/dd-bastion-readonly`; it intentionally does not grant Kubernetes Secret access or
patch/update/delete verbs.

`dd-bastion-readonly` was extended to also grant read access to `metrics.k8s.io` and `pods/log`
so the homepage "Live containers" cards can show per-container CPU/memory and stream logs without
needing exec. CPU and memory come from the cluster's `metrics-server` Argo CD app (kube-system)
and are read through the metrics aggregation API.

The browser terminal at `/bastion/terminal` is enabled in this Kubernetes deployment
(`BASTION_TERMINAL_ENABLED=true`) and the matching `pods/exec` `create` verb is granted by a
separate `ClusterRole`/`ClusterRoleBinding` named `dd-bastion-exec`. To revoke browser terminal
access without touching inventory routes, flip the env var back to `false` and remove the
`dd-bastion-exec` `ClusterRoleBinding` from `dd-bastion-rbac.yaml`. Read-only inventory + log
streaming continues to work even when `dd-bastion-exec` is detached.

## Recommended access model

The safe version of "one password for access" is not a public MCP server that can mint AWS access.
Keep the MCP server read-only, keep `dd-bastion` behind the authenticated gateway and WireGuard, and
use a long random `SERVER_AUTH_SECRET` only as a gateway/bastion bearer secret. AWS credentials stay
in AWS Secrets Manager, External Secrets, the EC2 instance profile, or a scoped CI/OIDC role; they
should not be returned by MCP tools or the bastion API.

For day-to-day operations:

1. Connect a WireGuard client created by the private wg-easy UI.
2. Query `dd-bastion` with `X-Bastion-Auth: $SERVER_AUTH_SECRET` for direct VPN access, or query
   the gateway `/bastion/...` paths with the operator `Auth` header / `dd_auth` cookie value.
   Use `/profile`, `/kubeconfig`, and `/runtime/deployments`.
3. Use the generated kubeconfig for read-only `kubectl get/list/watch` work.
4. Use the mobile Argo CD account for app status, resource inspection, logs, and explicit syncs.
5. Use normal key-based SSH or AWS Systems Manager Session Manager for host shell access. Do not
   make the Kubernetes MCP endpoint a public SSH/AWS credential broker.

## Secret setup

Create this JSON in AWS Secrets Manager before syncing the Argo app:

```json
{
  "INIT_USERNAME": "admin",
  "INIT_PASSWORD": "replace-with-a-long-random-password"
}
```

External Secrets Operator syncs it into the Kubernetes secret `dd-vpn-secrets` in the `vpn`
namespace. The `INIT_*` values are used only on the first start, before the SQLite database exists
on the PVC. Rotate UI credentials from the wg-easy admin UI after first boot, or delete the PVC if
you intentionally want a clean reinitialization.

The bastion service also expects `SERVER_AUTH_SECRET` in `dd/remote-dev/agent-secrets`, matching
the rest of the remote runtime.

The phone endpoint additionally needs a Cloudflare API token with `Zone:DNS:Edit` and
`Zone:Zone:Read`, restricted to the `fiducia.cloud` zone. Store it in AWS Secrets Manager at
`dd/remote-dev/cloudflare`, property `CLOUDFLARE_DNS_API_TOKEN`. The VPN app owns a dedicated
ExternalSecret named `argocd-mobile-cloudflare-dns-api-token` and a dedicated ClusterIssuer named
`letsencrypt-prod-dns01-argocd-mobile`. Their target Secret and ACME account key are distinct from
the optional public-gateway certificate migration, so Argo CD never gives two applications shared
ownership of the same certificate-control-plane resources.

## Bootstrap

1. Confirm `external-secrets-operator`, `dd-secrets`, and cert-manager are running.
2. Seed `CLOUDFLARE_DNS_API_TOKEN` as described above.
3. Update `INIT_HOST` in `dd-vpn.configmap.yaml` if the EC2 public IP or DNS name changes.
4. Apply the Argo CD app:

```bash
kubectl apply -f remote/argocd/apps/dd-vpn.application.yaml
```

5. Open UDP `51820` on the EC2 security group.
6. Watch the DNS-01 prerequisites and mobile certificate become healthy:

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
```

7. Open the wg-easy admin UI through a local port-forward:

```bash
kubectl -n vpn port-forward svc/dd-vpn-ui 51821:51821
```

Then visit `http://127.0.0.1:51821`, sign in, and create client configs.

After connecting a WireGuard client, query the bastion from the VPN:

```bash
curl -H "X-Bastion-Auth: $SERVER_AUTH_SECRET" \
  http://dd-bastion.vpn.svc.cluster.local:8111/profile

curl -H "X-Bastion-Auth: $SERVER_AUTH_SECRET" \
  http://dd-bastion.vpn.svc.cluster.local:8111/kubeconfig > dd-vpn.kubeconfig

KUBECONFIG=dd-vpn.kubeconfig kubectl get pods -A

curl -H "X-Bastion-Auth: $SERVER_AUTH_SECRET" \
  http://dd-bastion.vpn.svc.cluster.local:8111/runtime/deployments
```

## Phone setup for Argo CD

The mobile endpoint uses a dedicated local Argo CD account named `argocd-mobile`. Its RBAC allows:

- application/project/cluster/repository inspection
- pod-log reads
- explicit application syncs

It does not grant application-spec updates, Kubernetes resource deletion, pod exec, or resource
actions. The password is intentionally absent from Git and must be established interactively.

1. In wg-easy, set the client DNS server to `10.8.0.1`. `INIT_DNS` handles new installations, but
   wg-easy keeps initialized settings in SQLite, so update the UI setting and re-export an existing
   phone profile when necessary. An existing profile can also be edited so its `DNS` line is
   `10.8.0.1`.
2. Import/refresh the profile in the WireGuard phone app and activate the tunnel.
3. From a VPN-connected operator machine, establish the local-account password without putting it
   in shell history:

```bash
argocd login argocd-vpn.fiducia.cloud:8443 \
  --username admin \
  --grpc-web

argocd account update-password \
  --account argocd-mobile \
  --grpc-web
```

4. Verify the private name and TLS endpoint through the VPN:

```bash
nslookup argocd-vpn.fiducia.cloud 10.8.0.1
curl --fail --show-error --silent \
  https://argocd-vpn.fiducia.cloud:8443/healthz
```

5. In **Dashboard For Argo CD**, add:

```text
Server:   https://argocd-vpn.fiducia.cloud:8443
Username: argocd-mobile
Password: <the interactively established password>
```

The WireGuard tunnel must be active while the app is in use. On Android, strict Private DNS may
bypass a VPN-provided resolver; set Private DNS to `Automatic` if the private hostname does not
resolve. Do not work around a DNS or certificate problem by publishing port `8443`, opening the
existing Argo CD NodePort, or enabling an insecure/self-signed-certificate bypass.

## Certificate renewal behavior

cert-manager writes `Secret/vpn/argocd-mobile-tls`. The nginx sidecar watches the projected
certificate/key hash every 30 seconds and reloads nginx in-place when either changes. WireGuard
continues running during certificate renewal; there is no whole-pod restart and no phone tunnel
interruption.

The TLS proxy waits harmlessly when the secret is absent. This keeps the existing VPN and bastion
available while the Cloudflare token or issuer is being prepared. A missing/failed certificate
makes only the phone endpoint unavailable.

## Routing model

The first-boot config uses split-tunnel client routes:

- `10.8.0.0/24` for VPN clients and the VPN-local DNS/Argo CD listeners
- `10.96.0.0/12` for Kubernetes Services
- `10.244.0.0/16` for Kubernetes Pods

It advertises `10.8.0.1` as the DNS server. The DNS sidecar answers the private Argo CD hostname
itself and forwards other queries to kube-dns at `10.96.0.10`. For full-tunnel egress, change
`INIT_ALLOWED_IPS` to `0.0.0.0/0` before first boot, or update the setting in the UI after the VPN
has initialized.

This creates a VPC-like overlay into the cluster. It does not create or manage AWS VPC resources;
use Terraform or another AWS IaC path if the goal is a real AWS VPC.

“Bastion host” and “jump host” are used here as the same operational concept: a hardened hop for
private cluster access. This implementation keeps the hop as a narrow access broker by default; add
SSH or browser terminal access only if there is a concrete workflow that requires shell access.
