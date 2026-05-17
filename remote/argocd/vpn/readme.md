# `remote/argocd/vpn`

GitOps manifests for `dd-vpn`, a WireGuard VPN endpoint managed by
[`wg-easy`](https://wg-easy.github.io/wg-easy/latest/getting-started/). It creates a small
cluster-private VPN address space (`10.8.0.0/24`) and an admin UI for creating WireGuard clients.

The app uses `ghcr.io/wg-easy/wg-easy:15`; the wg-easy docs recommend pinning the major `15` tag
and avoiding `latest`, because `latest` still points at v14.

## What gets deployed

- Namespace: `vpn`
- Deployment: `dd-vpn`
- Public VPN listener: UDP `51820` on the EC2 node via Kubernetes `hostPort`
- Admin UI: `dd-vpn-ui.vpn.svc.cluster.local:51821`, ClusterIP only
- Persistent config: PVC `dd-vpn-config`, mounted at `/etc/wireguard`
- Secret source: AWS Secrets Manager key `dd/remote-dev/vpn-secrets`

The deployment uses a short privileged init container to set the network namespace sysctls that
WireGuard needs. The main wg-easy container gets Linux networking capabilities (`NET_ADMIN`,
`SYS_MODULE`) and a read-only mount of `/lib/modules` so WireGuard can use the host kernel module.
It runs as a single replica with `Recreate` rollout strategy so only one pod ever owns the host UDP
port and WireGuard state.

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

## Bootstrap

1. Confirm `external-secrets-operator` and `dd-secrets` are already synced.
2. Update `INIT_HOST` in `dd-vpn.configmap.yaml` if the EC2 public IP or DNS name changes.
3. Apply the Argo CD app:

```bash
kubectl apply -f remote/argocd/apps/dd-vpn.application.yaml
```

4. Open UDP `51820` on the EC2 security group.
5. Open the admin UI through a local port-forward:

```bash
kubectl -n vpn port-forward svc/dd-vpn-ui 51821:51821
```

Then visit `http://127.0.0.1:51821`, sign in, and create client configs.

## Routing model

The first-boot config uses split-tunnel client routes:

- `10.8.0.0/24` for VPN clients
- `10.96.0.0/12` for Kubernetes Services
- `10.244.0.0/16` for Kubernetes Pods

It also advertises `10.96.0.10` as the first DNS server so VPN clients can resolve cluster service
names through kube-dns. For full-tunnel egress, change `INIT_ALLOWED_IPS` to `0.0.0.0/0` before
first boot, or update the setting in the UI after the VPN has initialized.

This creates a VPC-like overlay into the cluster. It does not create or manage AWS VPC resources;
use Terraform or another AWS IaC path if the goal is a real AWS VPC.
