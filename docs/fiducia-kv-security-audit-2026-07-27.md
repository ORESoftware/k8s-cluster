# Fiducia KV secret-delivery security audit — 2026-07-27

## Scope

This audit traces every indexed Fiducia KV, API-key, and in-cluster load-balancer reference in
`ORESoftware/k8s-cluster`, plus the two upstream Rust services that define the relevant trust
contract:

- `fiducia-cloud/fiducia-load-balance.rs`;
- `fiducia-cloud/fiducia-auth.rs`;
- `daedalus-fab/fabrication-server.rs`.

The review covered GitOps manifests, External Secrets Operator resources, NetworkPolicies, secret
references, authentication defaults, KV key tenancy, at-rest protection, transport, storage
durability, and the existing repository tests. No live cluster credentials or cloud secret values
were read during this audit.

## Inventory

| Caller or component | Fiducia use | Credential path | Audit result |
| --- | --- | --- | --- |
| External Secrets Operator | Reads one application secret per KV key | `fiducia-eso-reader` from the independent cloud store | Store existed, but had no namespace conditions or admission guard |
| `fiducia-auth` | Persists API-key records and org indexes under `__auth/` | Shared trusted-hop secret | Client sent a node-only header that the LB strips; durable KV calls failed closed |
| `dd-fabrication-server` | Optional NATS secret overlay under `secrets/daedalus/*` | `FIDUCIA_API_KEY` from a Kubernetes Secret | Direct legacy reader; retain only until ESO migration, and keep replicas at zero |
| `dd-build-server` | Lock/lease coordination | Fiducia API key | Coordination, not secret delivery |
| `dd-contract-service` | Lock/lease/idempotency coordination | Fiducia API key | Coordination, not secret delivery |
| `dd-billing-server` | Lock/lease coordination | Fiducia API key in its application repository | Coordination, not secret delivery |
| `fiducia-node` | Encrypted Raft KV | Versioned encryption keyring from the cloud store | Encryption configured fail-closed; Raft data still uses `emptyDir` |
| Fiducia platform workloads | Trust, JWT, pepper, database, Supabase, and CSRF secrets | Previously assumed pre-created Kubernetes Secrets | Several references were optional or not declaratively bootstrapped |

No active application `ExternalSecret` on the audited `main` revision references
`ClusterSecretStore/dd-fiducia-kv`. The store was therefore an unguarded capability awaiting its
first consumer rather than an established per-application secret-delivery fleet.

## Findings and disposition

### Critical — auth storage identity did not satisfy the LB trust contract

`fiducia-auth` sent `x-fiducia-internal-auth` and an org header directly to the load balancer. The
load balancer deliberately strips the internal node header from inbound requests and accepts a
pre-verified identity only through its authenticated trusted-hop contract. Scoped `/v1/kv` routes
therefore rejected the auth service before durable API-key storage could work.

**Fix:** the upstream auth client now presents the trusted-hop proof plus a fixed service key ID,
its storage org, and only `kv:read kv:write`. Tests assert the exact headers on both GET and CAS PUT,
and assert that the node-only header is absent.

### High — cluster-wide ESO reader was not constrained by namespace or object policy

A `ClusterSecretStore` credential can otherwise be referenced by any namespace allowed to create an
`ExternalSecret`. Because the reader key is organization-wide, arbitrary remote keys would be
reachable through a malicious or mistaken object.

**Fix:** add a namespace condition and a fail-closed `ValidatingAdmissionPolicy`/binding. The policy:

- admits the store only in namespaces labelled `dd.dev/fiducia-kv-secrets=enabled`;
- permits Argo CD or cluster administrators as object authors;
- rejects `dataFrom` bulk extraction;
- requires an explicitly enumerated `spec.data` list;
- binds object and target names to the annotated workload;
- requires `creationPolicy: Owner` and `deletionPolicy: Retain`;
- enforces `k8s/<namespace>/<workload>/<ENV_VAR>` with `secretKey == ENV_VAR`.

### High — required Fiducia trust material was optional or undeclared

The cluster manifests assumed several Kubernetes Secrets already existed, and some trust/database
references were `optional: true`. Missing credentials could silently disable trusted-hop checks or
leave services in an ambiguous partial configuration.

**Fix:** declaratively bootstrap four narrowly enumerated runtime Secrets from the independent cloud
recovery root, make the security-critical references required, make LB auth explicit, disable legacy
unpeppered API-key hashes, and require Supabase service-role synchronization for the auth service.
The PR must not merge until the documented cloud objects and properties exist.

### High — Raft state is ephemeral

The node StatefulSet mounts `/var/lib/fiducia` from `emptyDir`. A pod replacement can erase one
replica's local Raft state; correlated replacement or node loss can destroy the KV authority. ESO's
last materialized Kubernetes Secret is not a complete or authoritative backup.

**Disposition:** staged follow-up. Changing an existing StatefulSet from `emptyDir` to
`volumeClaimTemplates` is not a safe in-place patch. The migration needs backups, restore tests,
quorum-aware rollout, PVC sizing/reclaim policy, and rollback evidence before GitOps switches the
volume shape.

### Medium — in-cluster KV traffic is plaintext HTTP

Bearer credentials and returned secret values cross the pod network without transport encryption.
NetworkPolicy and application-layer authentication reduce reachability but do not provide
confidentiality against node/network compromise.

**Disposition:** staged follow-up. The LB already supports TLS. Production rollout must issue a
cluster-trusted certificate, mount it, add an HTTPS service port, configure ESO `caProvider`, move
each direct client, reject non-probe plaintext traffic, and test rollback.

### Medium — a workload-controlled label widened LB ingress

Any default-namespace workload able to apply `dd.dev/fiducia-client=true` could reach the LB.
Authentication still applied, but the label was not a strong network identity boundary.

**Fix:** replace the generic selector with explicit application labels for the audited callers.
Future callers require an intentional NetworkPolicy review.

### Medium — runtime source is mutable

Several Fiducia pods clone public repositories from `main` and compile during startup. A restart can
therefore execute code different from the reviewed GitOps revision, and the network egress needed for
cold builds remains broad.

**Disposition:** staged supply-chain follow-up. Build immutable images in CI, pin source commits and
base-image digests, generate SBOM/provenance, scan/sign the image, and deploy by digest.

## Validation added

The existing `fiducia-secret-delivery.test.ts` was present as a package script but absent from
`repo-checks`. It is now part of the static contract matrix after a pinned kubectl setup. It renders
the common secrets and Fiducia kustomizations and checks:

- namespace-gated store conditions and admission resources;
- exact key-policy invariants and no bulk `dataFrom`;
- all cloud-bootstrap ExternalSecrets and required secret references;
- explicit LB authentication and legacy-hash rejection;
- encrypted-KV keyring requirements;
- explicit LB NetworkPolicy callers and removal of the generic label;
- namespace opt-in and runbook disaster-recovery/TLS warnings;
- absence of literal-looking Fiducia API keys in rendered output.

The upstream auth PR runs rustfmt, clippy with warnings denied, all tests, and `cargo audit` in its
existing CI.

## Merge and rollout gates

1. Seed every cloud object/property in `docs/fiducia-secret-delivery.md` without placing values in
   Git, pull requests, logs, or shell history.
2. Merge and deploy the upstream `fiducia-auth.rs` trusted-service-identity fix before or atomically
   with the k8s manifest change.
3. Verify all bootstrap `ExternalSecret` resources and both `ClusterSecretStore` objects are Ready.
4. Render and diff the Argo application, then restart one stateless component at a time. Do not
   deliberately restart all Raft members together.
5. Exercise API-key create, introspect, rotate, revoke, and restart persistence through the LB.
6. Exercise an allowed ESO read and rejected cases for wrong namespace, wrong author, `dataFrom`,
   wrong target name, and cross-workload key path.
7. Confirm NetworkPolicy permits every listed caller and denies an unlisted pod.
8. Keep the direct fabrication overlay at zero replicas until its keys are migrated to the guarded
   ESO convention and its pod no longer receives a Fiducia reader key.
