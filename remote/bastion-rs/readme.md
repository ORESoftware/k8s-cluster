# `remote/bastion-rs`

Small Rust bastion/access-broker service for the cluster VPN.

It is intentionally an authenticated HTTP service rather than a broad SSH shell. Once an operator
connects to the WireGuard VPN, they can query the bastion service for:

- `/profile` or `/config` - VPN, service CIDR, pod CIDR, DNS, and Kubernetes API connection info.
- `/kubeconfig` - a read-only kubeconfig backed by the pod's Kubernetes service account token.
- `/runtime/deployments` - live Deployment/Pod/container inventory for the managed runtime services,
  including the Rust WebRTC and Gleam WebSocket surfaces.
- `/healthz` - unauthenticated readiness check.

Protected routes accept `X-Bastion-Auth`, `X-Server-Auth`, `Auth`, or `Authorization: Bearer ...`
with the shared `SERVER_AUTH_SECRET`.

The terminal code path is compiled but disabled by default in Kubernetes, and the shipped RBAC does
not grant `pods/exec`. The Kubernetes manifests live in `remote/argocd/vpn/` because the bastion
remains a narrow access broker even when the public gateway proxies authenticated `/bastion/...`
browser requests to it.
