# Dynamic node provisioning — going from 1 box to an elastic 3+ node fleet

_Written 2026-07-20. The concrete pattern for running 3+ machines and adding/removing nodes
on demand. Companion to [cluster-scaling-architecture.md](cluster-scaling-architecture.md)._

## The one architectural rule

> **Never autoscale a control-plane node.** Fixed control plane, elastic worker pool.

Your Hetzner HA cluster today makes all 3 nodes **stacked-etcd control planes AND workers**
(`remote/hetzner/setup-cluster-ha.sh`: "All 3 nodes are stacked-etcd control planes … AND
workers"). That is a fine *HA* design and a bad *elastic* one — an autoscaler that adds or
removes an etcd member is corrupting quorum, not scaling. etcd wants a fixed, odd membership
(3), changed only deliberately.

So the target is two distinct pools:

```
┌─ control plane (FIXED, 3 nodes, stacked etcd, never autoscaled) ─┐
│  dd-k8s-fsn1 · dd-k8s-nbg1 · dd-k8s-hel1                         │
│  behind private LB dd-cp-lb 10.20.0.5:6443                       │
└──────────────────────────────────────────────────────────────────┘
┌─ worker pool (ELASTIC, 0..N, autoscaled, disposable) ────────────┐
│  dd-k8s-worker-*  ← cluster-autoscaler adds/removes these        │
└──────────────────────────────────────────────────────────────────┘
```

Taint the control-plane nodes once workers exist, so app pods drain onto the elastic pool and
the CP nodes do only control-plane work.

## Where you actually are

| | State |
|---|---|
| Distribution | **kubeadm** (not k3s), Cilium CNI, stacked etcd |
| HA topology | `setup-cluster-ha.sh` → `fsn1, nbg1, hel1` — all EU network zone, private L2 net, private LB control-plane endpoint ✅ |
| Legacy topology | `create-cluster.sh` → control-plane **ash** + workers hil/fsn1, WireGuard-meshed — cross-continent control path ⚠️ retire this |
| Join mechanism | manual: `kubeadm token create --print-join-command` on CP1, then `eval` with `--control-plane --certificate-key` — **every join is a control-plane join; there is no worker-only path** |
| cloud-init | installs kubelet/kubeadm/kubectl and holds them, then **deliberately does not join** ("that's orchestrated across all") |
| hcloud CCM | **not installed anywhere** ❌ |
| Autoscaler | none |
| Node provisioning IaC | none — `remote/terraform/` only covers cloudflare/r2 and aws/airbyte-s3 |

## The four gaps to close

### 1. Install the hcloud cloud-controller-manager (blocking prerequisite)

Without the CCM, three things silently break:

- **No `topology.kubernetes.io/zone` labels.** Every `topologySpreadConstraint` keyed on zone
  becomes a no-op — you get the illusion of spreading with none of the behavior.
- **No node lifecycle.** When a server is deleted, its `Node` object lingers as `NotReady`
  forever. cluster-autoscaler depends on the CCM to reap those.
- Kubelet must run with `--cloud-provider=external` for the CCM to take over.

This is the first thing to do, before any autoscaler. It also gives you the
`instance.hetzner.cloud/provided-by` and region/zone labels that scheduling depends on.

### 2. Add a worker-only join path

`setup-cluster-ha.sh` only ever joins nodes as control planes. You need the plain form:

```bash
kubeadm join dd-cp-lb:6443 \
  --token <bootstrap-token> \
  --discovery-token-ca-cert-hash sha256:<ca-hash> \
  --cri-socket unix:///run/containerd/containerd.sock
```

Note what's *absent*: no `--control-plane`, no `--certificate-key`. That's the whole
difference between "new etcd member" and "disposable worker."

### 3. Make a node join itself on boot

An autoscaler creates a server and walks away — nothing runs your orchestration script. So
the join has to live in the node's user-data.

Extend `cloud-init.yaml` with a `runcmd` that performs the worker join above. The wrinkle is
the token: **kubeadm bootstrap tokens expire after 24h by default**, so a node booted on day
three joins nothing and you get a silent capacity failure.

Three ways to handle it, in order of preference:

1. **Token-minting on scale-out** — a small controller (or the same automation that edits the
   node pool) issues a fresh short-TTL token and rewrites the pool's user-data. Most secure,
   most moving parts.
2. **Long-TTL token (`--ttl 0`) confined to the private network.** Simple and what most
   Hetzner setups do. Acceptable *only* because the control-plane endpoint
   (`dd-cp-lb 10.20.0.5:6443`) is private-network-only — the token is useless from the public
   internet. Rotate it on a schedule.
3. Bake a short-lived token into an image — don't; it expires with the image.

Whichever you pick, the CA cert hash is stable and safe to embed:
`openssl x509 -in /etc/kubernetes/pki/ca.crt -pubkey -noout | openssl pkey -pubin -outform DER | openssl dgst -sha256`.

### 4. Run cluster-autoscaler with the hcloud provider

Deploy `cluster-autoscaler` (as an ArgoCD Application under `remote/argocd/`, like the rest of
the platform layer) with the hcloud cloud provider. A node pool declares the server type,
location, image, and the self-joining user-data from step 3.

Key settings:

| Flag | Value | Why |
|---|---|---|
| `--scale-down-unneeded-time` | `10m` | avoid thrashing on bursty traffic |
| `--scale-down-utilization-threshold` | `0.5` | remove nodes under 50% requested utilization |
| `--expander` | `least-waste` | pick the pool that leaves least idle capacity |
| `--balance-similar-node-groups` | `true` | keep spread even across fsn1/nbg1/hel1 |
| `--skip-nodes-with-local-storage` | `true` | don't evict pods with `emptyDir` data you care about |
| `--skip-nodes-with-system-pods` | `true` | protects kube-system singletons |

Define **one pool per location** (fsn1, nbg1, hel1) rather than one multi-location pool, so
the autoscaler can satisfy zone spread constraints deliberately.

## How scaling actually triggers

Worth being precise, because it's a common misconception:

**cluster-autoscaler does not watch CPU.** It watches for **pods that cannot be scheduled**
(`Pending` with `FailedScheduling`). The chain is:

```
traffic ↑ → HPA/KEDA adds replicas → no node has room → pod Pending
        → cluster-autoscaler provisions a node → node self-joins → pod schedules
```

So **pod autoscaling drives node autoscaling**. If your HPAs aren't set up, nodes will never
scale out no matter how loaded the box is. And this entire chain depends on pods declaring
`requests` — a pod with no requests "fits" anywhere, so it never goes Pending, so no node is
ever added while the box thrashes. Your requests coverage (~120 manifests) is good; the
tenant `LimitRange` backstops the rest.

Expect **~2–4 minutes** end-to-end on Hetzner (server create → cloud-init → apt → join →
Ready). Keep enough headroom that you're not waiting on it for user-facing traffic — that's
what `minReplicas` and a little slack capacity are for.

## Scale-down safety

Scale-down is where this breaks if the prerequisites are skipped. The autoscaler removes a
node only if it can evict **every** pod on it.

- **PDBs are mandatory** — 10 today. A Deployment with no PDB can block a drain, or worse, be
  fully evicted at once. `minAvailable: 1` at minimum.
- **Never let Raft/quorum members be consolidated** (`raft-consensus-rs`, `live-mutex`,
  fiducia's core where it runs). Hard anti-affinity + `maxUnavailable: 1` PDBs, and consider
  `cluster-autoscaler.kubernetes.io/safe-to-evict: "false"`.
- **PVCs pin pods to a location.** `csi.hetzner.cloud` volumes cannot move between fsn1/nbg1/
  hel1. Set `volumeBindingMode: WaitForFirstConsumer` on the `dd-block` StorageClass so the
  volume is created wherever the pod lands, not the reverse.
- **DaemonSets don't block drain** and shouldn't be counted as utilization.

## AWS variant

If you scale the AWS side instead, note that **Karpenter effectively requires EKS** — it is
not a fit for your current single-node kubeadm-on-EC2 host. So:

- **Move to EKS** → use Karpenter (`NodePool` + `EC2NodeClass`), which provisions right-sized
  instances directly from pending-pod shape in ~60s and consolidates aggressively. Best-in-class,
  and worth it if burst elasticity is the priority.
- **Stay on kubeadm-on-EC2** → use cluster-autoscaler against an **Auto Scaling Group**, with
  the same self-joining user-data pattern as Hetzner. Works, but you're hand-rolling what EKS
  gives you.

Mix spot into the elastic worker pool for CI runners, browser E2E, and batch. Never spot for
etcd or stateful quorum members.

## Rollout sequence

Do these in order; each depends on the last.

1. **Retire the ash/WireGuard topology.** Standardize on `setup-cluster-ha.sh`
   (fsn1/nbg1/hel1, one EU network zone, private LB endpoint). One control path, low latency.
2. **Install hcloud CCM.** Verify: `kubectl get nodes -L topology.kubernetes.io/zone` shows a
   real zone per node. Nothing below is meaningful until this passes.
3. **Install the hcloud CSI driver** and set `WaitForFirstConsumer` on `dd-block`.
4. **Phase 0 from the scaling doc** — CI-built images, drop the 121 hostPath mounts. *Adding
   nodes is pointless until pods can schedule on them.*
5. **Add a worker-only join path + self-joining cloud-init** (gaps 2 and 3).
6. **Provision 2 workers manually** and confirm pods actually land on them. This validates
   the join path before an autoscaler depends on it.
7. **Taint the 3 control-plane nodes**; confirm workloads drain to workers.
8. **Add PDBs + `topologySpreadConstraints`** to every multi-replica Deployment (currently 10
   PDBs and 1 spread constraint — the biggest correctness gap).
9. **Deploy cluster-autoscaler** with per-location pools, `minSize: 1`, a sane `maxSize`
   (start ~4/location), and the flags above.
10. **Test both directions:** load until pods go Pending and a node appears; then remove load
    and confirm a node is reclaimed after `scale-down-unneeded-time`. **Verify scale-*down*
    explicitly** — it's the half that silently doesn't work.

## Verification checklist

```bash
kubectl get nodes -L topology.kubernetes.io/zone,node-role.kubernetes.io/control-plane
kubectl get pods -A -o wide | awk '{print $8}' | sort | uniq -c   # spread across nodes?
kubectl get pdb -A                                               # every real app covered?
kubectl -n kube-system logs deploy/cluster-autoscaler | grep -i "scale.up\|scale.down"
kubectl get events -A --field-selector reason=FailedScheduling   # what's actually pending
```

A node that never leaves is as much a bug as a node that never arrives.
