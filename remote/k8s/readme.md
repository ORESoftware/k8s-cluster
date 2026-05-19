# `remote/k8s/` — bare-EC2 Kubernetes manifests for dd-dev-server

This is the v4 deployment shape: **vanilla Kubernetes on plain EC2 instances**, no EKS, no ECS. One
Deployment per thread (= per conversation), addressed by the thread UUID.

For the design rationale see
[`../../docs/dev-hybrid-chat-plan-v4-k8s.md`](../../docs/dev-hybrid-chat-plan-v4-k8s.md).

## Manifest layout

| File                                 | Apply order | Purpose                                                                                                     |
| ------------------------------------ | ----------- | ----------------------------------------------------------------------------------------------------------- |
| `00-namespace.yaml`                  | first       | `dd-dev` namespace + Pod-Security baseline                                                                  |
| `01-configmap.yaml`                  | first       | Non-secret runtime config (provider, ingest URLs, paths)                                                    |
| `02-secrets.template.yaml`           | first       | **Template** — copy to `02-secrets.yaml`, fill in, apply                                                    |
| `03-rbac.yaml`                       | first       | Two ServiceAccounts: `dd-control-plane` + `dd-thread-pod`                                                   |
| `04-network-policy.yaml`             | first       | Pod ingress narrows to :8080; egress allowlist (DNS, HTTPS-public, SSH-public — blocks private/IMDS pivots) |
| `05-resource-quota.yaml`             | first       | Cap namespace CPU/mem/pods/PVC so runaway threads can't starve kube-system on the single-node box           |
| `06-thread-pvc.template.yaml`        | per-thread  | Substituted by control plane on first dispatch                                                              |
| `07-thread-deployment.template.yaml` | per-thread  | Substituted by control plane                                                                                |
| `08-thread-service.template.yaml`    | per-thread  | Substituted by control plane                                                                                |
| `09-thread-ingress.template.yaml`    | per-thread  | Per-thread path-based Ingress on a shared host (substituted by control plane)                               |
| `10-idle-reaper-deployment.yaml`     | optional    | Rust worker pod that sweeps idle threads and scales Deployments to `replicas=0`                            |
| `11-cron-service-configmap.template.yaml` | optional | Linux cron job definitions (`crond`) for sweep hooks and other scheduled shell tasks                        |
| `12-cron-service-deployment.yaml`    | optional    | Generic cron-service pod (Alpine + `crond`) that runs scripts from the cron ConfigMap                      |

The "first" files are applied once at cluster bring-up. The "per-thread" templates are instantiated
by `createK8sPod()` in
[`src/lib/server/remote-dev/container-registry.ts`](../../src/lib/server/remote-dev/container-registry.ts)
the first time a brand-new threadId arrives at `/api/admin/remote-dev/dispatch`.

## Cluster bring-up on Amazon Linux 2023

The dd-k8s-node AMI ([`../ami/k8s-dev-node.pkr.hcl`](../ami/k8s-dev-node.pkr.hcl)) ships with
`kubeadm`, `containerd`, `cilium`, `helm`, `argocd`, and a copy of `bootstrap-cluster.sh` symlinked
at `/usr/local/bin/dd-bootstrap-cluster`. The two-step bring-up below assumes you launched from
that AMI.

If you are starting from a plain Amazon Linux 2023 EC2 instance instead, use
[`../ec2/bootstrap-amazon-linux-2023-k8s.sh`](../ec2/bootstrap-amazon-linux-2023-k8s.sh) first.
It installs the same minimal Kubernetes prerequisites on the host, then delegates back to
`../ami/bootstrap-cluster.sh` so both paths converge on these manifests.

### 1. Provision the EC2 host

Single-node v4: control plane and workload pods run on the same EC2 box. No multi-node, no separate
workers. Use the AMI built from [`../ami/k8s-dev-node.pkr.hcl`](../ami/k8s-dev-node.pkr.hcl) so
kubeadm, containerd, and the language runtimes are pre-baked.

| Role            | Instance     | Why                                                                                                                        |
| --------------- | ------------ | -------------------------------------------------------------------------------------------------------------------------- |
| single-node × 1 | `x8i.2xlarge` | 8 vCPU / 128 GiB — the closest x86 fit to the target 100-120 GiB RAM band, with enough headroom for kube-system, ingress-nginx, ArgoCD, and the host-side `remote/dev-server` controls page |

Sizing notes:

- `t3.xlarge` (4 vCPU / 16 GiB) works for prereqs-only smoke tests — fine for validating the bootstrap path.
- For real thread pods plus the host-side `remote/dev-server`, stay on the larger x86 shape (`x8i.2xlarge` or a comparable 128 GiB class) before considering multi-node — single-node is simpler to operate and the workload is bursty.
- If you scale the box, edit the matching values in
  [`05-resource-quota.yaml`](./05-resource-quota.yaml) so the quota reflects allocatable capacity.

NAT gateway for image pulls / GitHub / Anthropic. IMDSv2 with `http_tokens=required` so a
compromised pod can't pivot to instance metadata.

**Required IAM on the EC2 instance role:**

- `AmazonEC2ContainerRegistryReadOnly` — pull the dd-dev-server image from ECR.
- `AmazonEBSCSIDriverPolicy` — let the in-cluster EBS CSI driver create / attach / detach EBS
  volumes for thread-pod PVCs. Without this, every PVC sits in `Pending` forever and pods never
  start.

(Both are AWS-managed policies; attach to the instance profile before launching from the AMI.)

### 2. Run the bootstrap script

Clone the repo on the host so the bootstrap can apply manifests from this directory, then fill in
real secret values:

```bash
sudo dnf install -y git
git clone https://github.com/ORESoftware/k8s-cluster.git ~/dd-next-1
cd ~/dd-next-1/remote/k8s

# Real values for ANTHROPIC_API_KEY, OPENCODE_API_KEY, GH_PAT, SUPABASE_*,
# REMOTE_DEV_REAPER_SECRET, etc.
cp 02-secrets.template.yaml 02-secrets.yaml
$EDITOR 02-secrets.yaml

sudo bash ~/dd-next-1/remote/ami/bootstrap-cluster.sh
```

That single command brings up the whole stack, in order:

1. starts `containerd`, disables swap
2. runs `kubeadm init` with `kube-proxy` skipped (Cilium replaces it)
3. configures `kubectl` for the invoking `ec2-user`
4. untaints the control-plane node so workloads schedule on it
5. installs **Cilium CNI** (eBPF — gives you NetworkPolicy enforcement; without it
   [`04-network-policy.yaml`](./04-network-policy.yaml) is silently a no-op)
6. installs **ArgoCD** on NodePort 30443 (browser cluster visualization at
   `https://<node-ip>:30443`)
7. installs **ingress-nginx** + **cert-manager** + **AWS EBS CSI driver** + a `gp3` default
   `StorageClass`
8. applies `00-`, `01-`, `02-`, `03-`, `04-`, `05-` to the `dd-dev` namespace
9. extracts the `dd-control-plane` SA token and prints the four Vercel env vars

The `06-/07-/08-/09-` template files are NOT applied here — they're instantiated on demand by the
control plane on the first dispatch for a new threadId.

Optional schedulers (apply manually if desired):

```bash
# Rust reaper worker
kubectl apply -f 10-idle-reaper-deployment.yaml

# Linux cron service (copy template first)
cp 11-cron-service-configmap.template.yaml 11-cron-service-configmap.yaml
kubectl apply -f 11-cron-service-configmap.yaml
kubectl apply -f 12-cron-service-deployment.yaml
```

### 3. Wire Vercel to the cluster

The bootstrap script prints the four cluster-auth values; the remaining two are operator-set (image
digest + ingress host). Paste all six into Vercel project env vars (Production + Preview):

```
# --- Auth (printed by bootstrap-cluster.sh) ---
K8S_API_SERVER=https://<node-ip>:6443
K8S_NAMESPACE=dd-dev
K8S_SA_TOKEN=<long base64-decoded string>
K8S_INSECURE_TLS=true   # acceptable for self-signed certs inside a private VPC
# ^ The Next.js control plane uses undici with a per-call dispatcher when
# this is true, so self-signed kube-apiserver certs work for the single-node
# kubeadm path. For a stricter production posture, trust the cluster CA with
# NODE_EXTRA_CA_CERTS or put a TLS-terminating reverse proxy with a public cert
# in front of the API.

# --- Workload config (operator-set) ---
K8S_POD_IMAGE=<account>.dkr.ecr.<region>.amazonaws.com/dd-dev-server:<digest>
K8S_INGRESS_HOST=agents.example.com
```

`K8S_POD_IMAGE` is the ECR-pinned image the orchestrator launches per thread. `K8S_INGRESS_HOST` is
the shared host you DNS-pointed at ingress-nginx in step 4 — the orchestrator builds per-thread
Ingress rules at `/dd-thread/<short>` on this host, and the browser-side SSE URL in
`getPublicDockerBaseUrl` is constructed from it.

To re-extract the SA token later (rotation, lost tmux scrollback, etc.):

```bash
kubectl -n dd-dev get secret dd-control-plane-token \
  -o jsonpath='{.data.token}' | base64 -d; echo
```

### 4. DNS + TLS

Point `agents.example.com` (or whatever you'll substitute as `{INGRESS_HOST}` in
[`09-thread-ingress.template.yaml`](./09-thread-ingress.template.yaml)) at the LoadBalancer / NLB
fronting your ingress-nginx Service. Then provision the shared TLS cert ONCE — per-thread Ingresses
just reference the resulting `dd-threads-tls` Secret. Two options:

**(a) cert-manager + Let's Encrypt** (auto-renewing):

```bash
kubectl apply -f - <<'EOF'
apiVersion: cert-manager.io/v1
kind: ClusterIssuer
metadata: { name: letsencrypt-prod }
spec:
  acme:
    server: https://acme-v02.api.letsencrypt.org/directory
    email: ops@example.com
    privateKeySecretRef: { name: letsencrypt-prod }
    solvers:
      - http01:
          ingress: { class: nginx }
---
apiVersion: cert-manager.io/v1
kind: Certificate
metadata:
  name: dd-threads-tls
  namespace: dd-dev
spec:
  secretName: dd-threads-tls
  issuerRef: { name: letsencrypt-prod, kind: ClusterIssuer }
  dnsNames: [agents.example.com]
EOF
```

cert-manager renews the cert automatically and refreshes the secret in place — per-thread Ingresses
keep working without touching them.

**(b) Hand-imported wildcard cert:**

```bash
kubectl -n dd-dev create secret tls dd-threads-tls \
  --cert=path/to/wildcard.crt \
  --key=path/to/wildcard.key
```

Either way the operator owns the cert lifecycle, decoupled from the per-thread Ingress churn.

## What happens on a new dispatch

```
Vercel /dispatch
    │
    ├─→ Redis: dd:thread:<uuid>     (fast path — sub-ms)
    │      hit?  yes → got pod URL, forward request, done.
    │            no  → fall through ↓
    │
    ├─→ K8s API: list pods labelSelector=dd/threadId=<uuid>
    │      hit?  yes → pod exists; cache URL in Redis, forward.
    │            no  → fall through ↓
    │
    └─→ K8s API: create PVC, Deployment, Service, Ingress from
        templates with {THREAD_ID} substituted; wait for readiness probe;
        cache URL in Redis; forward request.
```

After the first dispatch, subsequent prompts on the same threadId go through path 1 (Redis hit) —
sub-millisecond resolution.

## Worker broker migration

Today, the REST API may still broker calls to the pinned Node.js worker Deployment because workers
can be asleep at `replicas=0` and need lifecycle-aware wakeup before task delivery. That behavior is
allowed during the migration.

The long-run home for this responsibility is the always-on `dd-agent-worker-broker` Deployment in
`remote/argocd/dd-next-runtime`. It is a broker/dispatcher, not a general reverse proxy: the UI calls
`/api/agent-worker/threads/<threadId>/tasks`, the broker publishes the task to NATS JetStream,
emits the wakeup subject, direct-posts to `dd-thread-<short>:8080/tasks` only when that worker is
already healthy, and otherwise scales the worker Deployment back to `1`. Once this path is proven,
the REST API should keep data/API ownership and shed worker wake/dispatch/stream duties.

## Sleeping a thread

Triggered by the control-plane idle reaper (not in-pod self-exit):

```bash
kubectl -n dd-dev scale deployment dd-thread-<short-id> --replicas=0
```

The pod terminates. The PVC stays. Next dispatch on that threadId finds the Deployment with
`replicas=0`, scales it back to 1, and the PVC remounts to the new pod with the workspace exactly
where it was.

The per-thread container sets `IDLE_TIMEOUT_MS=0` so workload pods do not SIGTERM themselves. Sleep
policy is centralized in `/api/admin/remote-dev/reaper/sweep`, which checks active task state +
thread activity before scaling to zero.

## Idle Reaper and Cron Service

You can run either scheduler shape:

1. `10-idle-reaper-deployment.yaml` — Rust binary loop (`remote/idle-reaper-rs`) posting to the
   sweep endpoint every `REAPER_INTERVAL_SECONDS`.
2. `11` + `12` — Linux cron-service pod (`crond`) that executes shell jobs (default: once/minute
   sweep call via `curl`).

Both require:

- `REMOTE_DEV_REAPER_SECRET` in `dd-agent-secrets`
- `REAPER_SWEEP_URL` pointing to:
  `https://<vercel-host>/api/admin/remote-dev/reaper/sweep`

## Ending a thread

Triggered by the `End thread` button in `/u/admin/remote-dev` →
`POST /api/admin/remote-dev/threads/<id>/end` → `deleteK8sPod(threadId)`:

```bash
kubectl -n dd-dev delete ingress dd-thread-<short-id>
kubectl -n dd-dev delete deployment dd-thread-<short-id>
kubectl -n dd-dev delete service dd-thread-<short-id>
kubectl -n dd-dev delete pvc dd-thread-<short-id>
```

All workspace state gone. The `agent/<uuid>` branch + open PR on GitHub remain — the human
reviews + merges/closes them as normal.

## Image rebuild cadence

Every 3 days at 04:00 ET, `.github/workflows/cron-rebuild-dev-server-image.yml` rebuilds the image
with a fresh `git clone` + `pnpm install` and pushes to ECR. New thread pods pull `:latest` so they
start with a recent baseline; the per-pod `git fetch + checkout --hard origin/dev` is then a small
delta.

To force a rebuild:

```bash
gh workflow run cron-rebuild-dev-server-image.yml --ref dev
```

Existing pods keep running their pinned digest until they restart.

## Operational quick reference

```bash
# Watch all thread pods
kubectl -n dd-dev get pods -l app.kubernetes.io/component=thread-pod -w

# Tail a specific thread's logs
kubectl -n dd-dev logs -f deployment/dd-thread-<short-id>

# Inspect a thread's workspace (cd into the running pod)
kubectl -n dd-dev exec -it deployment/dd-thread-<short-id> -- bash

# Force-sleep a thread (returns at next dispatch)
kubectl -n dd-dev scale deployment dd-thread-<short-id> --replicas=0

# Force-delete (irreversible — workspace + PVC gone)
kubectl -n dd-dev delete ingress,deployment,svc,pvc dd-thread-<short-id>

# Health of the routing layer end-to-end
curl https://agents.example.com/dd-thread/<short-id>/healthz \
  -H "X-Server-Auth: $REMOTE_DEV_SERVER_SECRET"
```
