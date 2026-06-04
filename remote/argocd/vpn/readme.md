# `remote/argocd/vpn`

GitOps manifests for `dd-vpn`, a WireGuard VPN endpoint managed by
[`wg-easy`](https://wg-easy.github.io/wg-easy/latest/getting-started/), plus `dd-bastion`, a
Rust access broker/jump-host service reachable through that VPN. Together they create a small
cluster-private VPN address space (`10.8.0.0/24`), an admin UI for creating WireGuard clients, and
an authenticated place to retrieve cluster access profiles.

The app uses `ghcr.io/wg-easy/wg-easy:15`; the wg-easy docs recommend pinning the major `15` tag
and avoiding `latest`, because `latest` still points at v14.

## What gets deployed

- Namespace: `vpn`
- Deployment: `dd-vpn`
- Deployment: `dd-bastion`
- Public VPN listener: UDP `51820` on the EC2 node via Kubernetes `hostPort`
- Admin UI: `dd-vpn-ui.vpn.svc.cluster.local:51821`, ClusterIP only
- Bastion/access broker: `dd-bastion.vpn.svc.cluster.local:8111`, ClusterIP only
- Persistent config: PVC `dd-vpn-config`, mounted at `/etc/wireguard`
- Secret source: AWS Secrets Manager key `dd/remote-dev/vpn-secrets`
- Bastion auth source: AWS Secrets Manager key `dd/remote-dev/agent-secrets`, synced into
  `dd-bastion-secrets`

The deployment uses a short privileged init container to set the network namespace sysctls that
WireGuard needs. The main wg-easy container gets Linux networking capabilities (`NET_ADMIN`,
`SYS_MODULE`) and a read-only mount of `/lib/modules` so WireGuard can use the host kernel module.
It runs as a single replica with `Recreate` rollout strategy so only one pod ever owns the host UDP
port and WireGuard state.

`dd-bastion` is not a broad public SSH server. It is an authenticated Rust HTTP service that
operators reach through WireGuard or the gateway-proxied `/bastion/...` paths:

- `GET /healthz` - unauthenticated health check.
- `GET /profile` or `/config` - VPN endpoint, DNS, service CIDR, pod CIDR, and cluster API info.
- `GET /kubeconfig` - read-only kubeconfig using the `dd-bastion` service account token.
- `GET /runtime/deployments` - live managed Deployment, Pod, and container inventory.

Protected routes accept `X-Bastion-Auth`, `X-Server-Auth`, `Auth`, or `Authorization: Bearer ...`
with `SERVER_AUTH_SECRET`. The generated kubeconfig is bound to
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
2. Query `dd-bastion` with `X-Bastion-Auth: $SERVER_AUTH_SECRET` for `/profile`, `/kubeconfig`, and
   `/runtime/deployments`.
3. Use the generated kubeconfig for read-only `kubectl get/list/watch` work.
4. Use normal key-based SSH or AWS Systems Manager Session Manager for host shell access. Do not
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

“Bastion host” and “jump host” are used here as the same operational concept: a hardened hop for
private cluster access. This implementation keeps the hop as a narrow access broker by default; add
SSH or browser terminal access only if there is a concrete workflow that requires shell access.
