# Local Kubernetes platform contract

Tracking: `DEN-1032`.

This directory defines the portable admission contract for laptop-hosted Kubernetes test environments. It intentionally does **not** define AWS, Hetzner, GCP, Azure, or other provider resources.

## Peer authorities

`main.tsp` and `authored.schema.json` are independently maintained authorities at the same level. Neither is generated from nor subordinate to the other. `ORESoftware/typespec-json-schema-validator` (`tjsv`) emits a TypeSpec-derived comparison witness and fails closed when the two authorities diverge.

The CI contract pins the exact validator revision and runs the differential instance corpus under `instances/RuntimeProfile/` in both directions.

## Runtime meanings

- `colima-kind`: Colima supplies the Linux VM and Docker daemon required on macOS; kind supplies disposable Kubernetes node containers. `three-node` means one control-plane node plus two worker containers. This is the default workload/manifests test target.
- `multipass-k3s`: one Ubuntu 24.04 Multipass VM hosts one K3s cluster. Use multiple profiles/VMs when independent guest kernels, reboot behavior, or site boundaries matter. This is the host/bootstrap/recovery test target.

Plain Docker is deliberately not a valid runtime kind: a working container daemon is necessary for the kind lane but does not itself prove Kubernetes admission or API readiness.

## Safety boundary

Every admitted profile must set both:

```json
{
  "cloudMutationPolicy": "denied",
  "cloudWrites": false
}
```

The semantic verifier rejects extra keys, so provider credentials cannot be smuggled into this profile shape. Runtime bootstrap scripts operate only on dedicated Colima profiles, kind clusters, Multipass VMs, and ignored `tmp/local-k8s/` evidence.

The namespace migration work in `DEN-2786` remains a separate gate. Its current kind smoke proves ownership-aware namespace behavior. This contract supplies the reusable local host/runtime layer that can consume those manifests without weakening that migration contract.
