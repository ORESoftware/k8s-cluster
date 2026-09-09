# Local Kubernetes platform contract

Tracking: `DEN-1032`.

This directory defines the portable admission contract for laptop-hosted Kubernetes test environments. It intentionally does **not** define AWS, Hetzner, GCP, Azure, or other provider resources.

## Peer authorities

`main.tsp` and `authored.schema.json` are independently maintained authorities at the same level. Neither is generated from nor subordinate to the other. `ORESoftware/typespec-json-schema-validator` (`tjsv`) emits a TypeSpec-derived comparison witness and fails closed when the two authorities diverge.

The CI contract pins the exact validator revision and runs the differential instance corpus under `instances/RuntimeProfile/` in both directions. It also replays the same TJSV check and requires an identical deterministic receipt, then independently mutates the authored JSON Schema and the TypeSpec authority and requires both mutations to stop evaluation with retained findings. The negative tests operate on ignored scratch copies and verify that neither authored source changes.

## Cross-runtime enforcement

The peer-authority contract is the admission boundary for every runtime that consumes a `RuntimeProfile`: Rust, TypeScript/JavaScript, Dart/Flutter, Go, Gleam, or another language must not invent a handwritten incompatible copy. Generated language types and wire/client projections are downstream evidence, not new authorities, and are admitted only after TJSV parity and differential instance validation pass for the exact source revision.

When this contract grows an RPC surface, the TypeSpec lane may generate Protobuf/gRPC evidence and must use the exact-pinned TJSV Protobuf compatibility gate before promotion. The JSON Schema/OpenAPI lane remains an independent authored authority for JSON/HTTP interfaces and write clients. This local runtime profile currently has no RPC operations, so the repository does not manufacture a synthetic Protobuf authority merely to satisfy a tooling checkbox.

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
