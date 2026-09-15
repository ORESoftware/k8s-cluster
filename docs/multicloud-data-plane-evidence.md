# Retained evidence for the multi-cloud data plane

The manifests under `remote/argocd/multicloud-data-plane` are deployment intent. Helm/Kustomize rendering proves that the intent is structurally valid; it does not prove that AWS, GCP, and Azure are privately connected, that one CockroachDB cluster spans the three Kubernetes clusters, that all NATS gateways are healthy, or that backup and failure recovery work.

A rollout therefore produces a redacted `dd.multicloud-data-plane.v1` JSON artifact and binds the reviewed artifact to an exact `ORESoftware/k8s-cluster` commit with a deterministic SHA-256 digest.

## Security boundary

Evidence contains identifiers, booleans, counts, timestamps, private CIDRs/IPs, and SHA-256 fingerprints only. It must not contain certificate PEM, private keys, client credentials, cloud credentials, GitHub/Linear tokens, URLs with embedded credentials, model prompts, or secret values. The verifier rejects common secret-bearing field names and credential/certificate patterns.

CockroachDB and NATS use separate trust anchors. The CockroachDB CA fingerprint must be identical across the three providers, and the NATS CA fingerprint must be identical across the three providers, but the two fingerprints must differ. Fiducia remains a separate identity and network trust realm: it is not an ORES NATS gateway member and does not share the NATS CA, account, or JetStream domain.

## Required proof

The artifact records:

- exact source repository, 40-character Git SHA, collection mode, and bounded evidence window;
- three distinct Kubernetes cluster UIDs, the expected regions, three failure zones per provider, Kubernetes 1.30 or newer, encrypted `dd-block` storage, and tested snapshot restore;
- two narrow private peer CIDRs per provider, all six private DNS names resolved only to private addresses, and failed public reachability probes for CockroachDB SQL/RPC and NATS gateways;
- one CockroachDB cluster ID, nine live nodes, three nodes per region, zero unavailable or under-replicated ranges, SQL TLS, wrong-CA rejection, complete-region survival, encrypted backup, isolated restore, and measured RPO/RTO;
- three independent regional R3 JetStream groups with unique `ORES_AWS`, `ORES_GCP`, and `ORES_AZURE` domains, six directed mTLS gateway connections, route/client TLS, unknown-cluster rejection, redelivery, DLQ, snapshot restore, partition recovery, mirror lag, and zero duplicate protected effects;
- an explicit cross-region durability mechanism. Gateways route Core NATS interest; durable cross-region state still needs JetStream mirrors/sources and/or a transactional outbox;
- continued authority of `messaging/dd-nats` during shadowing and canary migration. This evidence contract does not authorize deleting or converting the bootstrap StatefulSet;
- the Fiducia application/API gateway boundary, including replay protection and idempotency.

Production-mode evidence is stricter than canary evidence: it requires a clean-room CockroachDB restore plus NATS operator/account JWT isolation and subject ACL verification.

## Verify

The verifier takes paths and reviewed digests through environment variables so no ad-hoc credential-bearing flag surface is introduced:

```bash
DD_MULTICLOUD_EVIDENCE_PATH=./result/multicloud-evidence.json \
  node remote/tools/multicloud-data-plane-evidence.mjs
```

The command emits `evidence_sha256`. Record that digest in the reviewed activation ticket and verify it again before promotion:

```bash
DD_MULTICLOUD_EVIDENCE_PATH=./result/multicloud-evidence.json \
DD_MULTICLOUD_EXPECTED_SHA256=<64-hex-reviewed-digest> \
  node remote/tools/multicloud-data-plane-evidence.mjs
```

The digest is calculated from recursively key-sorted JSON after removing the self-declared `evidence_sha256` field. A stale or contradictory artifact exits non-zero.

`remote/argocd/multicloud-data-plane/evidence.example.json` is synthetic canary evidence used by CI. Its green result is not live cloud proof.

## Promotion sequence

1. Record the exact reviewed Git revision and confirm the child Argo CD Applications are still manual.
2. Satisfy the cluster, storage, private-routing, DNS, and external-secret prerequisites in all three providers.
3. Bootstrap CockroachDB once, join the other two regions, and run the range, TLS, region-loss, backup, and clean-room restore probes.
4. Validate each regional NATS R3 group locally, then open gateways and exercise mTLS rejection, unknown-cluster rejection, disconnect/recovery, mirrors/sources, redelivery, DLQ, and snapshot restore.
5. Capture and verify the artifact in canary mode. Shadow traffic without changing bootstrap authority.
6. Add NATS operator/account JWT isolation and subject ACL proof, repeat a clean-room restore, verify production-mode evidence, and review its digest.
7. Promote one subject/database workload family at a time. Rollback returns clients to the previous endpoint and retains all new stateful volumes for diagnosis.

Local render success, synthetic evidence, and CI success are source-quality signals only. They do not justify production activation without the retained live artifact.
