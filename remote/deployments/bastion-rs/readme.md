# `remote/deployments/bastion-rs`

Small Rust bastion/access-broker service for the cluster VPN.

It is intentionally an authenticated HTTP service rather than a broad SSH shell. Once an operator
connects to the WireGuard VPN, they can query the bastion service for:

- `/profile` or `/config` - VPN, service CIDR, pod CIDR, DNS, and Kubernetes API connection info.
- `/kubeconfig` - a read-only kubeconfig backed by the pod's Kubernetes service account token.
- `/runtime/deployments` - live Deployment/Pod/container inventory for the managed runtime services,
  including the Rust WebRTC and Gleam WebSocket surfaces. Each pod and container row also carries a
  CPU + memory snapshot pulled from `metrics.k8s.io/v1beta1` (metrics-server) when available.
- `/healthz` - unauthenticated readiness check.
- `/terminal` - browser exec terminal for the managed-deployment allowlist. Disabled at the code
  level by default (`BASTION_TERMINAL_ENABLED=false`) and gated at the Kubernetes layer by the
  dedicated `dd-bastion-exec` `ClusterRoleBinding`.
- `/logs/ws` - websocket-streamed `kubectl logs -f` for the same allowlist, scoped to the chosen
  container. Uses the `pods/log` read in the read-only ClusterRole; no exec is required.

Protected routes accept `X-Bastion-Auth`, `X-Server-Auth`, `Auth`, or `Authorization: Bearer ...`
with the shared `SERVER_AUTH_SECRET`.

The terminal code path is compiled and **disabled by default in code**: the env fallback for
`BASTION_TERMINAL_ENABLED` is `false`, so a misconfigured deployment that forgets to opt in still
returns `403`. The Kubernetes deployment in `remote/argocd/vpn/dd-bastion.deployment.yaml`
deliberately enables the terminal and binds the dedicated `dd-bastion-exec` `ClusterRole` (which
contains exactly one verb, `create` on `pods/exec`). The read-only `dd-bastion-readonly`
`ClusterRole` never grows mutation verbs - splitting the two makes it cheap to roll back terminal
access without disturbing the inventory routes that the homepage relies on.

The Kubernetes manifests live in `remote/argocd/vpn/` because the bastion remains a narrow access
broker even when the public gateway proxies authenticated `/bastion/...` browser requests to it.
