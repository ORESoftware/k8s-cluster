# Cluster scaling architecture — target topology and the path to get there

_Written 2026-07-20. Scope: how to run ~5–10 apps on `ORESoftware/k8s-cluster` with
horizontal scaling, dynamic node add/remove, and pod autoscaling. Companion to
[gitops-boundary-audit.md](gitops-boundary-audit.md) and
[app-deploy-contract.md](app-deploy-contract.md)._

## TL;DR

1. **Do not go multi-region.** One Kubernetes cluster cannot span AWS regions (etcd needs
   single-digit-ms RTT). For 5–10 apps, multi-region buys nothing and costs latency, egress,
   and data gravity. **One cluster, one region, 3 AZs.**
2. **The blocker is not infrastructure — it's the workload pattern.** 121 manifests mount
   `hostPath` (mostly `/home/ec2-user/codes/dd/dd-next-1`) and 15 build from
   `rust:1.95-bookworm` in-pod at startup. Those pods can only ever schedule on the one box
   that has that source tree. Karpenter, cluster-autoscaler, and extra nodes do **nothing**
   until this is fixed. **This is the gate for everything else.**
3. **AWS today is a single-node cluster**, not a cluster: `remote/ec2/README.md` provisions a
   single-node kubeadm host (`x8i.2xlarge`, ~128 GiB) and everything co-locates on it.
   Hetzner is already the more mature topology (control-plane in ash, workers in hil + fsn1).
4. **Recommendation: Hetzner as the primary scaled cluster, AWS/EKS only if you need managed
   control plane or AWS-native services.** Detail and numbers below.

## Reality check — what exists today

| Signal | Today |
|---|---|
| AWS topology | **single-node kubeadm** (`x8i.2xlarge` ~128 GiB), all pods co-located |
| Hetzner topology | multi-node, `ccx53`, control-plane ash + workers hil/fsn1 |
| Node fungibility | **broken** — 121 hostPath manifests, 15 in-pod cargo builds |
| Prebuilt images | only **10** manifests (ghcr.io / ECR) |
| Node autoscaling | **none** (no cluster-autoscaler, no Karpenter) |
| Pod autoscaling | 3 HPAs, 4 KEDA ScaledObjects — KEDA 2.19 + metrics-server already installed ✅ |
| Resource requests | good coverage (~120 manifests declare `requests`) ✅ |
| Disruption-readiness | 10 PDBs, 5 podAntiAffinity, **1 topologySpreadConstraint** ⚠️ |
| Storage | `dd-block` per cloud: `ebs.csi.aws.com` / `csi.hetzner.cloud` / `pd.csi.storage.gke.io` |

The good news: requests coverage, KEDA, metrics-server, ESO, and the ArgoCD multi-cluster
layout (`remote/argocd/clusters/{aws,hetzner,gcp}`) are already in place. The scaling
foundation is mostly there — it's the hostPath/in-pod-build pattern that pins everything down.

## Answering the topology question directly

### Multi-region? No.

A single cluster cannot span regions. Multi-region means **N independent clusters**, which
costs you: N control planes, cross-region data replication, egress fees, and N× the
operational surface. Worth it only for (a) legal data residency, (b) real user latency
requirements on distant continents, or (c) region-loss DR with an RTO tighter than
"re-provision from Git."

You have none of those for 5–10 apps. And you already have the mechanism if you ever do —
`remote/argocd/clusters/<cloud>/` plus an ApplicationSet cluster generator deploys the same
apps to N clusters from one commit.

### Multi-AZ? Yes — this is the real win.

3 AZs in one region gives you node-failure and AZ-failure survival with **one** control plane
and no cross-region latency. This is the correct target.

### How many machines for 5–10 apps?

**3 nodes minimum, 5 as the comfortable steady state, autoscaling to ~8 under burst.**

- 3 is the floor: it's the smallest count that tolerates losing one node while keeping quorum
  for HA control plane and letting PDBs actually evict safely.
- Sizing: with ~10 apps × 2–3 replicas at ~200m CPU / 512Mi each, plus the observability stack
  (Prometheus, Loki, Grafana, Tempo — not cheap), budget roughly **24–32 vCPU and 96–128 GiB
  total** across the fleet. Three 8-vCPU/32 GiB nodes ≈ your current single box's capacity,
  but survivable and schedulable.
- Prefer **more, smaller nodes** over fewer large ones: better bin-packing, cheaper
  autoscaling granularity, smaller blast radius. Avoid going below ~4 vCPU/16 GiB per node or
  the observability stack won't fit alongside apps.

## Provider decision: Hetzner vs AWS

| | Hetzner | AWS (EKS) |
|---|---|---|
| Cost | **~5–10× cheaper** for equivalent vCPU/RAM | expensive; the single `x8i.2xlarge` alone is a large monthly line item |
| Control plane | self-managed (k3s/kubeadm) — you own upgrades/etcd | **managed**, ~$73/mo/cluster |
| Failure domains | separate datacenters (fsn1/hel1/ash); cross-DC latency is real | true AZs, <2ms, designed for spanning |
| Volumes | `csi.hetzner.cloud` — **location-bound**, a PVC cannot move between locations | EBS — AZ-bound (same constraint, but AZs are closer) |
| Node autoscaling | `cluster-autoscaler` + hcloud provider — works, less mature | **Karpenter** — best-in-class, provisions right-sized nodes in ~60s |
| Spot/preemptible | none | spot instances, big savings for burst |

**Recommendation:** stay on **Hetzner as the primary scaled cluster**. You already run
multi-node there successfully, it's dramatically cheaper, and for 5–10 apps you do not need
EKS's managed control plane badly enough to pay 5–10× for it.

Caveat that decides it the other way: if you want **true dynamic burst** (scale 3→8 nodes in
a minute and back down), Karpenter on EKS is meaningfully better than cluster-autoscaler on
hcloud. If burst elasticity is a hard requirement, use AWS.

**Important Hetzner constraint:** keep all nodes of one cluster in **one location group**
(e.g. all `fsn1`+`nbg1`+`hel1` in the EU network zone) so private networking is low-latency
and volumes are usable. Do **not** spread one cluster across ash (US) and fsn1 (EU) — the
current control-plane-in-ash / workers-in-hil+fsn1 layout has cross-continent latency in the
control path and should be consolidated.

## Phased plan

### Phase 0 — make nodes fungible (THE PREREQUISITE)

Nothing below works until this is done. No autoscaler can help while a pod requires one
specific machine's filesystem.

- **Build images in CI, not in pods.** Each app repo publishes `ghcr.io/<org>/<app>:<sha>`.
  10 manifests already do this (`benefactor-backend-rs`, `canonical-*`) — that's the pattern
  to copy. Kills both the hostPath source mounts and the 15 in-pod `cargo build`s.
- **Delete `hostPath` source mounts.** Legitimate remaining hostPath uses are node-agent
  patterns only (log collection, node exporters) — those should be DaemonSets.
- **Replace hostPath caches with PVCs or emptyDir.** Build caches (`/home/ec2-user/.cache/*`)
  become either a registry-cached layer or an `emptyDir`.
- **Payoff beyond scaling:** cold starts stop being multi-minute cargo builds, which is
  currently a real availability problem (the audit notes pods "briefly unavailable after a
  redeploy … cold in-pod cargo build").

Track this per-app; it is the single highest-leverage work in this document.

### Phase 1 — multi-node with correct spreading

- Stand up **3 nodes across 3 failure domains** in one region/location group.
- Ensure every node carries `topology.kubernetes.io/zone` (cloud-controller-manager does this;
  hcloud CCM must be installed for Hetzner).
- Add **`topologySpreadConstraints`** to every multi-replica Deployment. You currently have
  **1** — this is the biggest gap. Without it, 3 replicas can all land on one node and an AZ
  loss takes the app down despite "3 replicas".

  ```yaml
  topologySpreadConstraints:
    - maxSkew: 1
      topologyKey: topology.kubernetes.io/zone
      whenUnsatisfiable: ScheduleAnyway   # DoNotSchedule once capacity is proven
      labelSelector:
        matchLabels: { app: <app> }
  ```
- Add **PDBs** for every app that matters (10 today). `minAvailable: 1` at minimum, or nodes
  can never be drained safely — which blocks scale-down and upgrades.

### Phase 2 — pod autoscaling (mostly already available)

metrics-server and KEDA 2.19 are installed; requests coverage is good. So HPA works today.

- **HPA for CPU/memory-bound apps.** Currently only 3. Extend to the web/API tier.
  Target ~70% CPU utilization; set `minReplicas: 2` for anything user-facing.
- **KEDA for event-driven apps** — you already have 4 ScaledObjects and NATS in-cluster.
  Queue-depth scaling (`dd-fabrication-server` off its NATS subject) is a far better signal
  than CPU for worker-shaped workloads, and KEDA can scale **to zero**, which HPA cannot.
- **Requests are the contract.** HPA percentages and the scheduler both key off `requests`.
  The tenant `LimitRange` (see `remote/argocd/projects/_tenant-scaffold.template.yaml`)
  guarantees a default so nothing lands unrequested.
- Avoid HPA and VPA on the same workload (they fight).

### Phase 3 — dynamic node scaling

Once pods are fungible and spread-aware, node autoscaling is straightforward.

**On AWS — Karpenter** (preferred): watches for unschedulable pods and provisions a
right-sized node in ~60s; consolidates and removes underutilized nodes automatically.
Mix spot for burst/batch with on-demand for the baseline.

**On Hetzner — `cluster-autoscaler` with the hcloud provider**: define node pools per server
type, set `--scale-down-unneeded-time` (~10m) and `--scale-down-utilization-threshold` (~0.5).

Either way, scale-down is the part that breaks without Phase 1: the autoscaler can only remove
a node if it can evict every pod on it, which requires PDBs that permit eviction and no
un-replicated singletons pinned to that node.

### Phase 4 — multi-cluster (only if genuinely needed)

If DR or regional latency ever justifies it, add a second cluster and deploy via an
ApplicationSet cluster generator over `remote/argocd/clusters/`. Do **not** stretch one
cluster across regions.

## Scale-down and cost control

- **Karpenter consolidation** / cluster-autoscaler scale-down reclaims idle nodes.
- **KEDA scale-to-zero** for queue workers and anything idle overnight.
- **`idle-reaper-rs`** already exists in `remote/deployments/` — fold it into this story
  rather than duplicating its function.
- **Spot/preemptible** for CI runners, browser E2E, batch and training — never for stateful
  Raft members.
- **Right-size from real data:** Prometheus already scrapes everything; compare
  `requests` against observed usage and trim. Over-requesting is the most common cause of
  "we need more nodes."

## Stateful caveats (do not skip)

- **PVCs are AZ/location-bound.** A pod with an EBS or hcloud volume can only reschedule into
  the same AZ/location. Use `volumeBindingMode: WaitForFirstConsumer` on the `dd-block`
  StorageClasses so the volume is created where the pod actually lands.
- **Raft/quorum workloads** (fiducia node/brain, `raft-consensus-rs`, `live-mutex`) need
  **hard** anti-affinity across nodes and an odd member count. Never let an autoscaler
  consolidate two members onto one node. Give them PDBs with `maxUnavailable: 1`.
- Per the audit, **fiducia's Raft core belongs on fiducia-infra's own clusters**, not here —
  only its stateless api/web servers are tenants of this cluster.
- Databases: keep Postgres/Supabase external. Do not autoscale a database into existence.

## What to do first

1. **Phase 0 on the top 3–5 apps by traffic** — CI-built images, drop hostPath. Without this
   nothing else moves.
2. **Add `topologySpreadConstraints` + PDBs** to every multi-replica Deployment (biggest
   correctness gap: 1 spread constraint across ~99 Deployments).
3. **Consolidate the Hetzner cluster into one location group**, 3 nodes, hcloud CCM installed
   so zone labels exist.
4. **Extend HPA/KEDA** to the web/API tier; queue-depth scaling for workers.
5. **Then** enable cluster-autoscaler/Karpenter — by which point it will actually work.
