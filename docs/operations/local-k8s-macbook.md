# MacBook local Kubernetes and EC2-style VM testing

Tracking: `DEN-1032`. Namespace migration tracking remains `DEN-2786`.

## Decision matrix

Use two complementary local lanes instead of pretending one topology proves everything:

| Lane | Host boundary | Kubernetes nodes | Best for |
| --- | --- | --- | --- |
| Colima + kind | one Colima Linux VM | one or three kind node containers | Kubernetes manifests, admission, scheduling, service/RBAC behavior, fast disposable CI-like testing |
| Multipass + K3s | one Ubuntu VM per profile | one K3s node in each VM | Linux provisioning, systemd, reboot/recovery, independent guest kernels, EC2-style host behavior |

Docker Engine by itself is **not** a Kubernetes cluster. On macOS it still needs a Linux VM behind it. The Colima lane makes that VM explicit and then creates Kubernetes with kind.

Minikube remains valid for interactive development, but this repository standardizes its reproducible contract tests on kind because topology and pinned node images are explicit in source. Do not claim that a multi-node kind cluster proves independent EC2-host failure behavior; use separate Multipass VMs for that boundary.

## Prerequisites

For the fast workload lane:

```bash
brew install colima docker kind kubectl
```

For the EC2-style host lane:

```bash
brew install --cask multipass
```

Apple Silicon should normally use the `arm64` profiles. Intel Macs use `amd64`. Cross-architecture emulation is a separate test and is not silently enabled by these scripts.

## Contract verification

The runtime profiles are governed by independently authored TypeSpec and JSON Schema authorities:

- `contracts/local-k8s-platform/main.tsp`
- `contracts/local-k8s-platform/authored.schema.json`

CI pins `ORESoftware/typespec-json-schema-validator` and runs differential validation over the committed instance corpus. Local semantic admission is also fail-closed:

```bash
LOCAL_K8S_PROFILE=contracts/local-k8s-platform/instances/RuntimeProfile/valid/colima-kind.json \
  python3 scripts/local-k8s/verify_profile.py

LOCAL_K8S_PROFILE=contracts/local-k8s-platform/instances/RuntimeProfile/valid/multipass-k3s.json \
  python3 scripts/local-k8s/verify_profile.py
```

The verifier rejects Docker-only profiles, mutable kind tags, Kubernetes/image version drift, extra fields, any cloud-write authorization, and a multi-node topology hidden inside a single Multipass VM profile.

## Fast lane: Colima + kind

The committed standard profile creates one control-plane and two worker node containers using the same digest-pinned kind node image as the current `DEN-2786` local namespace verifier.

```bash
export LOCAL_K8S_PROFILE="$PWD/contracts/local-k8s-platform/instances/RuntimeProfile/valid/colima-kind.json"
bash scripts/local-k8s/bootstrap-colima-kind.sh
```

The script creates only dedicated local resources:

- Colima profile `ores-mac-kind`;
- Docker context `colima-ores-mac-kind`;
- kind cluster `mac-kind`;
- ignored evidence and kubeconfig under `tmp/local-k8s/mac-kind/`.

Use the isolated kubeconfig rather than changing an existing cloud context:

```bash
kubectl --kubeconfig tmp/local-k8s/mac-kind/kubeconfig get nodes -o wide
kubectl --kubeconfig tmp/local-k8s/mac-kind/kubeconfig get pods -A
```

Do not run the bootstrap over an existing same-named kind cluster. The script refuses reuse instead of mutating an unknown cluster.

## Host lane: Multipass + K3s

```bash
export LOCAL_K8S_PROFILE="$PWD/contracts/local-k8s-platform/instances/RuntimeProfile/valid/multipass-k3s.json"
bash scripts/local-k8s/bootstrap-multipass-k3s.sh
```

The standard profile creates `ores-mac-vm-a`, an Ubuntu 24.04 VM with four CPUs, 8 GiB memory, 60 GiB disk, and a version-pinned K3s install. The installer script itself is retained inside the VM with a SHA-256 receipt so a test run records exactly what was executed.

Operate Kubernetes through the guest boundary:

```bash
multipass exec ores-mac-vm-a -- sudo k3s kubectl get nodes -o wide
multipass exec ores-mac-vm-a -- sudo k3s kubectl get pods -A
```

Exercise host recovery rather than just pod recovery:

```bash
multipass restart ores-mac-vm-a
multipass exec ores-mac-vm-a -- sudo systemctl is-active k3s
multipass exec ores-mac-vm-a -- sudo k3s kubectl wait --for=condition=Ready nodes --all --timeout=180s
```

For three independent local sites, create three distinct Multipass profiles/VMs. Do not encode `three-node` inside one Multipass profile; that would collapse the independent-host failure boundary that `DEN-1032` is meant to test.

## Relationship to `DEN-2786`

`DEN-2786` owns classification and ownership-aware migration of the legacy development namespaces. Its PR #1527 already includes a cloud-free kind smoke that proves the namespace inventory, manifest parity, canonical platform ConfigMap round trip, server-side admission, and least-privilege RBAC.

This `DEN-1032` layer does not replace that verifier and does not bulk-rewrite legacy names. It provides reusable local runtime/VM boundaries on which the migration verifier and future product-neutral recovery tests can run.

## Safety rules

- Both admitted profiles require `cloudMutationPolicy: denied` and `cloudWrites: false`.
- The local bootstrap scripts accept no cloud or repository credentials.
- They contain no AWS, GCP, Azure, Hetzner, Cloudflare, GitHub, or Linear mutation client calls.
- Existing same-named kind clusters and Multipass VMs are not overwritten.
- Runtime evidence stays below ignored `tmp/local-k8s/`.
- No script here authorizes production deployment, namespace cleanup, secret migration, or provider provisioning.
