# DD K8s on Hetzner Cloud

Hetzner port of the AWS provisioning under [`../ec2/`](../ec2/) and [`../ami/`](../ami/).
Each location gets one **single-node** cluster (kubeadm + Cilium + ArgoCD) — same
single-node, scale-up-not-out design as the EC2 path. A multi-region *single*
cluster is intentionally avoided: cross-continent etcd latency would wreck it.

## What it builds

Per server (CCX53 = 32 dedicated vCPU / 128 GB, Ubuntu 24.04):

- containerd + runc + CNI plugins (upstream GitHub binaries)
- kubeadm/kubelet/kubectl `1.31` (from the `pkgs.k8s.io` **deb** repo)
- `kubeadm init` with kube-proxy skipped
- **Cilium** CNI (eBPF; kube-proxy replacement + NetworkPolicy)
- **ArgoCD** on NodePort `30443`
- **local-path-provisioner**, exposed as a default StorageClass named **`gp3`**

### Port notes vs. the AWS path

| AWS (`../ec2`, `../ami`) | Hetzner (here) |
|---|---|
| Amazon Linux 2023, `dnf`/rpm | Ubuntu 24.04, `apt`/deb |
| AWS EBS CSI driver + `gp3` SC | local-path-provisioner aliased as `gp3` |
| EC2 security group | Hetzner Cloud Firewall (`dd-k8s-fw`) |
| Instance type `x8i.2xlarge` (8 vCPU/128 GiB) | `ccx53` (32 vCPU/128 GiB) |

The `gp3` alias means existing PVC manifests that reference `storageClassName: gp3`
keep working unchanged. `gp3` here is node-local disk, not network block storage —
swap in the Hetzner CSI driver (`hcloud-volumes`) if PVs must survive node replacement.

> There is **no 16 vCPU / 128 GB** shape on Hetzner Cloud — the vCPU:RAM ratio is
> fixed per line, so 128 GB ⇒ 32 vCPU (CCX53). `ccx43` is the 16 vCPU / 64 GB option.

## Usage

```bash
# 1. Authenticate (one time) — paste a Read & Write API token from
#    console.hetzner.cloud -> Project -> Security -> API Tokens
hcloud context create dd-hetzner

# 2. Create the fleet (Ashburn, Hillsboro, Falkenstein)
./create-cluster.sh
#    ...or one region / a different size:
SERVER_TYPE=ccx43 ./create-cluster.sh fsn1
```

`create-cluster.sh` registers your SSH key (`~/.ssh/id_hetzner.pub`), creates a
firewall locked to your current public IPv4 (tcp 22/6443/30443; `FIREWALL=0` to
skip), then creates one server per location with `cloud-init.yaml` as user-data.

## After boot (~5–10 min)

```bash
ssh root@<ipv4>
cloud-init status --wait                 # block until bootstrap finishes
kubectl get nodes                        # 1 Ready node
cilium status

# ArgoCD
open https://<ipv4>:30443                # user: admin
kubectl -n argocd get secret argocd-initial-admin-secret \
  -o jsonpath='{.data.password}' | base64 -d; echo
```

## Wiring GitOps (optional next step)

Point ArgoCD at this repo and use
[`../argocd/clusters/hetzner`](../argocd/clusters/hetzner/) as the cluster bootstrap overlay.
That overlay keeps the shared app manifests on the same `dev` branch as EC2/GCP, while selecting
the Hetzner storage and secret-store implementations.

The public gateway ingress for the current Hetzner load balancer lives in
[`dd-remote-gateway-ingress.yaml`](dd-remote-gateway-ingress.yaml) and serves
`https://hello.95-217-171-250.sslip.io/` through `dd-remote-gateway`.

## Teardown

```bash
for loc in ash hil fsn1; do hcloud server delete "dd-k8s-$loc"; done
hcloud firewall delete dd-k8s-fw
```
