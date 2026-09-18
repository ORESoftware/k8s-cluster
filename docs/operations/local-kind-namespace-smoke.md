# Cloud-free namespace migration smoke test

This runbook exercises the legacy-to-canonical ownership-aware namespace migration contracts against a real Kubernetes API without AWS, Hetzner, Argo CD, provider credentials, or application secrets.

The smoke proves that:

- the classifier tests, deterministic inventory, and migration manifest remain valid at the exact source commit;
- a fresh disposable kind cluster reaches API-server readiness using an isolated kubeconfig;
- the canonical `ores/` namespace contract round-trips through Kubernetes admission;
- the observability exporter's RBAC, ConfigMap, Service, and Deployment pass server-side admission dry-run;
- the exporter service account can list pods but cannot read Secrets;
- no cloud provider or secret backend is contacted.

It does not prove cloud load balancers, CSI volumes, instance metadata, IAM, provider controllers, host provisioning, reboots, or live application readiness.

## Prerequisites

Install Docker, kind, kubectl, Git, and Python 3. The script fails closed if a prerequisite or Docker daemon is unavailable. It also refuses to reuse an existing cluster name, so it cannot silently mutate an unrelated local cluster.

On macOS, Colima can provide the Linux VM and Docker runtime:

```bash
brew install colima docker kind kubectl
colima start ores-local --runtime docker --cpu 4 --memory 6 --disk 60
export DOCKER_CONTEXT=colima-ores-local

docker info
```

Docker Desktop is also acceptable; use its active Docker context instead of starting Colima.

## Run the same smoke used by CI

From the repository root:

```bash
bash scripts/ci/local-kind-smoke.sh
```

The default topology is one control-plane node. To exercise scheduling across two worker nodes on a sufficiently resourced Mac:

```bash
KIND_WORKERS=2 \
  bash scripts/ci/local-kind-smoke.sh
```

The CI workflow pins the kind binary by release checksum, pins the Kubernetes 1.32.8 node image by digest, checks out the exact pull-request head rather than the synthetic merge commit, and uploads the resulting evidence.

Override the node image only for an explicit compatibility test and record the replacement digest:

```bash
KIND_NODE_IMAGE='kindest/node:<version>@sha256:<digest>' \
  bash scripts/ci/local-kind-smoke.sh
```

Keep the cluster after a successful run for interactive inspection:

```bash
KIND_KEEP_CLUSTER=1 \
  KIND_CLUSTER_NAME=ores-namespace-audit \
  KIND_EVIDENCE_DIR="$PWD/.local-evidence/ores-namespace-audit" \
  bash scripts/ci/local-kind-smoke.sh

KUBECONFIG="$PWD/.local-evidence/ores-namespace-audit/kubeconfig" \
  kubectl get nodes -o wide
KUBECONFIG="$PWD/.local-evidence/ores-namespace-audit/kubeconfig" \
  kubectl get pods -A
kind delete cluster --name ores-namespace-audit
```

When `KIND_EVIDENCE_DIR` is omitted, the script creates a unique directory beneath `${RUNNER_TEMP:-${TMPDIR:-/tmp}}` and prints its path. Failure evidence includes kind logs, an API-server dump, and cluster resources. By default, the cluster is deleted whether the smoke passes or fails.

The script never calls `kubectl config use-context` and never writes to the normal `~/.kube/config`; every Kubernetes call uses the run-specific kubeconfig in the evidence directory.

## Optional VM isolation on macOS

Use Multipass when the purpose is to isolate Linux host behavior from the Colima VM or exercise the repository's real bootstrap scripts:

```bash
brew install --cask multipass
multipass launch 24.04 \
  --name ores-k8s-vm \
  --cpus 4 \
  --memory 6G \
  --disk 50G

multipass info ores-k8s-vm
multipass shell ores-k8s-vm
```

A VM is not required for the kind smoke. Do not substitute K3s for a production kubeadm/containerd bootstrap and then claim host-provisioning parity; run the repository's actual bootstrap path in the guest.

## Linux VM parity tier

Use a separate Ubuntu VM or physical host for the host-provisioning tier. Run the repository's actual bootstrap mechanism there, matching the production container runtime and Kubernetes distribution. That tier should verify system services, kernel and filesystem assumptions, firewall rules, reboot behavior, node rejoin, storage, rollback, and compatibility aliases.

A kind pass is not evidence that EC2- or Hetzner-specific metadata, IAM, storage, networking, or cloud-controller behavior works. Keep provider-dependent certification blocked while those providers are unavailable.

## Migration completion gate

A green local-cluster smoke verifies the tooling and Kubernetes admission surface; it does not mean the migration is complete. The migration parent remains open until every active legacy reference has an accountable classification, target, verification procedure, and rollback plan, and strict mode rejects any new unallowlisted legacy reference.
