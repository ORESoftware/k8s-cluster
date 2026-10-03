# Local Kubernetes platform contract

Tracking: `DEN-1032`, follow-up `k8s-cluster#1547`.

This directory defines the portable admission contract for laptop-hosted and credential-free CI Kubernetes test environments. It intentionally does **not** define AWS, Hetzner, GCP, Azure, or other provider resources.

## Peer authorities

`main.tsp` and `authored.schema.json` are independently maintained authorities at the same level. Neither is generated from nor subordinate to the other. `ORESoftware/typespec-json-schema-validator` (`tjsv`) emits a TypeSpec-derived comparison witness and fails closed when the two authorities diverge.

The CI contract pins the exact validator revision and runs the differential instance corpus under `instances/RuntimeProfile/` and `instances/RuntimeEvidence/` in both directions. It also replays the same TJSV check and requires an identical deterministic receipt, then independently mutates the authored JSON Schema and the TypeSpec authority and requires both mutations to stop evaluation with retained findings. The negative tests operate on ignored scratch copies and verify that neither authored source changes.

## RuntimeProfile and RuntimeEvidence

`RuntimeProfile` is desired runtime configuration. `RuntimeEvidence` is downstream evidence from one exact profile and one immutable source revision. Evidence carries the exact profile SHA-256 plus the same runtime, architecture, resource class, topology, Kubernetes version, artifact, and no-cloud-write policy.

`verify_evidence.py` adds cross-object policy that cannot be expressed by either one-object schema alone. It requires exact profile/evidence convergence, a full 40-character source revision, Kubernetes API readiness, server-side admission, pod-list RBAC, negative Secret RBAC, no provider writes, no secret inputs, and `cloudMutationPolicy=denied` / `cloudWrites=false`.

The `docker-kind` profile exists for credential-free GitHub-hosted exact-head execution. It is **not** a relaxation of the Docker-only negative case: a Docker daemon by itself is still not admitted. The valid CI lane creates a real kind cluster from the digest-pinned node image and emits RuntimeEvidence only after the Kubernetes API and RBAC checks succeed.

## Cross-runtime enforcement

The peer-authority contract is the admission boundary for every runtime that consumes these records. `language-boundaries.json` requires five distinct downstream identities:

- Rust / native;
- TypeScript / Node;
- Dart / Flutter;
- Go / native;
- Gleam / BEAM.

CI uses TJSV's current-input language-boundary gate. It rebuilds the parity-approved Contract IR from the exact checkout, binds each generated downstream evidence artifact to the exact TJSV parity receipt and Contract IR identity, and requires ingress plus egress validation for every mandatory runtime. Contract tests also prove that missing Gleam evidence, stale Go source revision, failed TypeScript ingress, and failed Dart egress all stop promotion.

Those generated artifacts and language-boundary receipts are evidence only. They do not become a third authored schema authority, and the boundary test does not pretend to compile every product consumer. Native consumer repositories still owe their own runtime/compiler tests before promotion; this manifest makes the required language/runtime set and fail-closed evidence contract explicit.

When this contract grows an RPC surface, the TypeSpec lane may generate Protobuf/gRPC evidence and must use the exact-pinned TJSV Protobuf compatibility gate before promotion. The JSON Schema/OpenAPI lane remains an independent authored authority for JSON/HTTP interfaces and write clients. This local runtime contract currently has no RPC operations, so the repository does not manufacture a synthetic Protobuf authority merely to satisfy a tooling checkbox.

## Runtime meanings

- `colima-kind`: Colima supplies the Linux VM and Docker daemon required on macOS; kind supplies disposable Kubernetes node containers. `three-node` means one control-plane node plus two worker containers. This is the default Mac workload/manifests test target.
- `docker-kind`: Docker supplies the container runtime and kind supplies Kubernetes. This is used for credential-free CI and other Linux hosts where a separate Colima VM is unnecessary. Docker without kind remains invalid.
- `multipass-k3s`: one Ubuntu 24.04 Multipass VM hosts one K3s cluster. Use multiple profiles/VMs when independent guest kernels, reboot behavior, or site boundaries matter. This is the host/bootstrap/recovery test target.

Plain Docker is deliberately not a valid runtime kind: a working container daemon is necessary for the kind lanes but does not itself prove Kubernetes admission or API readiness.

## Safety boundary

Every admitted profile and runtime evidence record must retain:

```json
{
  "cloudMutationPolicy": "denied",
  "cloudWrites": false
}
```

Runtime evidence additionally requires `providerWrites=false`, `secretInputs=false`, and `rbacGetSecrets=false`. The semantic verifiers reject extra keys, so provider credentials cannot be smuggled into these shapes. Runtime bootstrap/evidence scripts operate only on dedicated local runtimes, disposable kind clusters, Multipass VMs, and isolated evidence directories.

The namespace migration work in `DEN-2786` remains a separate gate. Its kind smoke proves ownership-aware namespace behavior. This contract supplies the reusable local host/runtime and evidence layer that can consume those manifests without weakening that migration contract.
