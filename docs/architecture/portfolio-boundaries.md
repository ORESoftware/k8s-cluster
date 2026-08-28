# Portfolio architecture boundaries

This document turns the current cross-organization architecture into a reviewable default
contract. The machine-readable authority is
[`portfolio-contract.json`](portfolio-contract.json). Its status is
`proposed-defaults`: merge only after owners agree that exceptions are explicit rather than
implicit.

## Dependency direction

```text
Flutter / browser / desktop / CLI / extension
                  |
                  v
       product SDK in *-clients
                  |
          +-------+--------+
          v                v
   product API       realtime gateway
   or thin web BFF   WebSocket/WebRTC
          |                |
          +-------+--------+
                  v
       product domain services
          |        |        |
          v        v        v
       Postgres   NATS    object storage
                  |
              workers/jobs

MCP server ------> product SDK/API, never product tables
Cross-product ---> published API/SDK or versioned event, never another database
Product service -> shared-auth for human identity
Worker/control --> Fiducia for leases, fencing, elections, and protected side effects
Product sync ----> Opto Sync for schema-agnostic reconciliation
Bulk transfer ---> signed object-storage URLs or File Tunnel
```

A dependency is legal because of the role it fulfills, not because two repositories happen
to be in the same organization or Postgres instance.

## Repository role vocabulary

Repository names should converge on the roles in the JSON contract:

- `*-interfaces` owns wire, event, configuration, and desired-state schema contracts.
- `*-clients` owns generated DTOs and transport SDKs.
- `*-api-server` owns the authenticated command/query boundary.
- `*-web-server` owns HTML, assets, browser routing, and at most a thin BFF.
- `*-worker` owns asynchronous execution and event consumption.
- `*-ctrl-server` or `*-control-plane` owns desired state and fleet policy, not execution.
- `*-sync` owns product-specific offline transport, encryption, and conflict policy while
  consuming Opto Sync's schema-agnostic merge core.
- `*-mcp-server` is a bounded, audited facade over the product SDK/API.
- `*-infra` owns external DNS, WAF, storage, IAM, and Terraform; it does not duplicate
  Kubernetes workloads.
- `*-monorepo` is generated inventory and immutable release pins, not a second source tree.
- `*-e2e` owns black-box conformance tests.

The ambiguous role `backend` should be retired when a repository is next split or renamed.
Use the actual responsibility: API, worker, scheduler, ingestion, processor, or control plane.

## API and data rules

1. A product service does not read or write another product's tables. Shared database
   infrastructure still uses separate schemas, roles, and migration authorities.
2. External commands and queries use HTTPS REST/JSON by default. WebSocket is the normal
   client realtime transport. WebRTC signaling stays a dedicated contract.
3. NATS JetStream is the default asynchronous integration path. Database mutation and
   outbox insertion are one transaction; consumers claim the message id in the same
   transaction as the side effect.
4. Direct Supabase access is an exception for simple user-owned rows with complete RLS,
   narrow grants, no privileged side effect, no competing API write path, and documented
   deletion/audit behavior.
5. gRPC requires an ADR showing that HTTP or NATS is materially insufficient.
6. Production builds do not depend on moving Git branches or undeclared sibling paths.
7. Provider-specific credentials, quota handling, retries, and webhook reconciliation stay
   in a product-owned adapter unless multiple products share the same semantic operation.
8. MCP servers call the product SDK/API. Direct database access would create an undocumented
   second business API and is forbidden by default.

## Platform ownership

- `ORESoftware/k8s-cluster` owns cluster-scoped resources, ArgoCD registration and tenancy,
  shared operators, NATS, secrets delivery, observability backends, and the durable worker
  platform. Application repositories own namespace-scoped workloads. Infra repositories own
  external cloud resources.
- `shared-auth` is the human identity, session, token-exchange, JWKS, and authentication
  assurance authority. Product authorization remains in each product. Fiducia machine and
  project authorization remains in Fiducia.
- `fiducia-cloud` owns leases, locks, fencing, elections, coordination, and fail-safe
  schedules. Its stateful Raft data plane stays on dedicated Fiducia multi-cloud clusters.
  Stateless customer/admin/API tenants may run on the shared cluster.
- `scintilla-run` is an ephemeral runtime target, not the durable workflow authority.
- `opto-sync` is a deterministic reconciliation primitive, not a product schema, key,
  transport, or deletion-policy authority.
- `file-tunnel` owns authenticated resumable byte transfer, not product metadata.
- `networking-components` owns low-level networking primitives. Fiducia and products retain
  topology and routing policy.
- `zed-pkg` owns immutable cross-language package graphs, lockfiles, and registry mirrors.
- `declarative-migrations` owns guarded database desired-state diff/review/apply tooling.
- `ORESoftware/mcp-rust-libs` owns reusable MCP transport hardening; product repositories own
  credentials, authorization, and mutations.

## Observability

Every deployable process must emit the resource attributes enumerated in the JSON contract
and propagate W3C trace context across HTTP, NATS, WebSocket, MCP, child processes, and
provider calls. Correlation and causation ids remain in event envelopes even when a trace is
not sampled.

Container stdout/stderr is a first-class log source. Instrumentation is explicit at process
and platform boundaries; runtime monkey-patching is not permitted. User, tenant, message,
audio, task, file, email, and phone identifiers are not Prometheus labels or Loki stream
labels.

Raw messages, audio, credentials, provider bodies, contact details, stream keys, prompts,
model output, health observations, and biometric templates do not enter ordinary logs,
metrics, or traces. Security, consent, ledger, administrative, fencing, device-command,
deletion, and key-rotation records belong in an append-only audit store with independent
retention and access policy.

## Deployment

The default release path is build once, test the exact artifact, generate SBOM and
provenance, sign, and promote the immutable digest across environments. ArgoCD registers the
application and enforces tenancy; it does not rebuild source.

Database desired state lives with the interface/schema authority. A reviewed migration job
runs before incompatible code. Destructive SQL is never approved by routine GitOps sync.

Production uses per-product namespaces, RBAC, quota, network policy, secret prefixes,
database roles, and NATS permissions. Fiducia Raft, media nodes, GPU workloads, and
drone/embedded control use specialized placement and failure policies rather than the
ordinary stateless API template.

## Exceptions and unresolved product decisions

An exception requires an ADR naming an owner, rationale, security review, and migration or
expiry date. The initial decisions that still need product-specific ADRs include:

- whether Sonus Auris raw audio is permanently device-only or may be uploaded as
  client-encrypted objects;
- whether StreemPilot remains P2P signaling or adopts TURN, SFU, compositor, recording, and
  RTMP relay roles;
- whether Memebank's current `mbk-*` repositories are legacy-v1, migration sources, or
  parallel authorities;
- whether reserved streaming organizations become products, editions, or remain names only;
- which products receive separate clusters/accounts because their sensitivity exceeds
  namespace isolation.

The contract test intentionally fails when a connected organization disappears from the
catalog, a reserved/research organization becomes a production dependency, an ambiguous
repository role is added, or a core fail-closed rule is weakened.
