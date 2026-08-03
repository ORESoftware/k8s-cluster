# Fiducia Production Safety Release Gate

| Field | Value |
|---|---|
| Linear owner | `DEN-1391` |
| Scope | Initial managed public beta capabilities in the v0.1 service contract |
| Decision | **Fail closed: no real-user launch until required rows pass** |
| Machine-readable matrix | [`production-safety-release-gate.json`](production-safety-release-gate.json) |

## 1. Purpose

Fiducia is a cross-service trust system. A secure node is not enough if the load balancer trusts spoofed headers, the auth cache accepts a revoked key too long, a restore rewinds fencing state, the secret-delivery path can cross namespaces, or production runs an unverified mutable artifact.

This gate maps system threats to executable tests and durable evidence. It consumes component work rather than declaring it done. A merged PR, green unit test, screenshot, or prose assertion does not by itself approve a row.

## 2. Protected assets

1. Tenant identity and membership: organization, project, environment, roles, scopes, credential versions, and revocation state.
2. Coordination authority: lock/lease ownership, fencing tokens, queue order, idempotency records, election terms, and committed revisions.
3. Secret confidentiality and integrity: encrypted KV values, path policy, encryption key versions, ESO delivery, and bootstrap roots.
4. Consensus durability: Raft log, snapshots/WAL, membership, term/index, and placement state.
5. Release integrity: source revision, dependency locks, generated interfaces, image digest, SBOM, provenance, configuration, and GitOps history.
6. Evidence integrity: audit events, traces, metrics, test artifacts, incident records, backups, and restore reports without sensitive payloads.

## 3. Trust boundaries and route classes

Every route in the approved beta maps to one of these required classes in the JSON matrix:

| Route class | Boundary | Examples |
|---|---|---|
| `dashboard_customer` | Customer browser/BFF → customer app/auth | Supabase session, `/v1/me`, API-key lifecycle |
| `dashboard_admin` | Operator browser/BFF → admin app/auth | trusted operator roles, local registry, break-glass |
| `public_data_plane` | Customer workload → edge/regional LB | API key/JWT, idempotency, lock/lease/KV/rate-limit routes |
| `trusted_edge_to_lb` | Edge → regional LB | verified identity headers plus edge shared secret |
| `lb_to_node` | Regional LB → data-plane node | stripped client credentials, injected org/scope/key identity, internal auth |
| `auth_internal` | LB/edge → auth introspection and key authority | `x-server-auth`, authoritative KV, bounded cache |
| `raft_peer` | Node ↔ node and brain ↔ brain | authenticated append/vote/snapshot/membership traffic |
| `sidecar_brain` | Node sidecar ↔ brain/control plane | health, membership, placement, topology |
| `secret_delivery` | Kubernetes/ESO/provider → guarded KV path | namespace/service account, path ACL, CA trust, generation |
| `observability` | Services/collectors → logs/metrics/traces/audit sinks | redaction, cardinality, tenant isolation, backpressure |
| `backup_restore` | Live state → external backup → clean cluster | encryption, immutability, key custody, monotonic restore |
| `build_release` | Source/dependencies → CI → registry → GitOps → runtime | locks, pins, SBOM, provenance, digest, policy |

A newly exposed route class is a release-blocking schema change until threat rows are added and validated.

## 4. Adversary model

The gate assumes:

- an unauthenticated internet caller can control headers, method, path encoding, query, body, origin, timing, concurrency, disconnects, and retries;
- a valid customer credential may be malicious, compromised, stale, over-scoped, or replayed against a different tenant/project/environment;
- a dashboard user may have a valid Supabase identity but no membership or operator role for the requested surface;
- an attacker may reach a node/LB/internal route directly because of a routing or firewall regression;
- a service, pod, laptop, network path, storage device, certificate, credential, telemetry sink, or entire site may fail, pause, partition asymmetrically, restart, or return late;
- build inputs, tags, generated contracts, deployment overlays, support artifacts, and backups may drift or be tampered with;
- operators make mistakes during incident response, migration, restore, and rollback.

## 5. Non-negotiable invariants

1. **Tenant isolation:** no successful operation can use a customer-controlled organization/project/environment/path identity that was not derived from verified authority.
2. **Plane separation:** customer keys, human sessions, operator roles, internal introspection secrets, trusted-hop secrets, and peer credentials are not interchangeable.
3. **Stale authority rejection:** after a newer fencing token or credential version exists, an older authority cannot perform an accepted protected mutation beyond its declared bound.
4. **Committed-state monotonicity:** restart, failover, restore, rollback, or migration cannot rewind committed revisions, fencing tokens, lease epochs, key versions, revocations, or audit sequence.
5. **Secret non-disclosure:** secret values and raw credentials never enter source control, issue trackers, logs, metrics, traces, browser persistence, exports, screenshots, or support evidence.
6. **Fail-closed readiness:** missing storage, quorum, identity, actor health, CA trust, policy, or required secret keeps the affected path unready or denied; it does not silently enter permissive mode.
7. **Immutable release:** runtime artifacts and configurations are traceable to an approved source/config commit and immutable digest with verified provenance.
8. **Independent recovery:** the service and its decryption/bootstrap roots can be rebuilt from Git, external backups, and independently recoverable custody material.

Any violation is an automatic no-go or immediate containment event.

## 6. Evidence requirements

Every required JSON row has:

- a stable test ID and route class;
- the threat and invariant being proven;
- an executable test description;
- the intended automation location;
- blocking Linear issues;
- a status and evidence list.

Allowed statuses:

- `not_started` — no credible execution evidence yet;
- `automated` — a test exists and passes in CI, but release/live evidence is still pending;
- `live_evidence_pending` — automation passed; exact-candidate environment proof is pending;
- `passed` — exact-candidate evidence exists and a reviewer has accepted it;
- `accepted_risk` — only for a waivable lower-severity gap with owner, rationale, expiry, containment, and evidence;
- `failed` — the gate is closed.

A `passed` row requires at least one durable URL or immutable artifact reference. `accepted_risk` requires a named risk owner, rationale, expiry date, containment plan, remediation issue, and reviewer evidence. Automatic no-go invariants may not use `accepted_risk`.

## 7. Execution phases

### Phase A — Static and CI proof

- schema and route coverage validation;
- dependency locks and immutable sibling revisions;
- Nix/container/OCI parity;
- generated-contract conformance;
- auth, header, scope, path, CORS/cookie, encoding, idempotency, and redaction negative tests;
- rendered production-manifest policy checks;
- SBOM, provenance, secret scanning, and vulnerability policy.

### Phase B — Ephemeral adversarial integration

- distinct tenant/org/project/environment fixtures;
- direct-node and trusted-header spoof attempts;
- stale introspection/JWT/revocation behavior;
- concurrency, retry, cancellation, ambiguous-response, and path-normalization tests;
- process/pod/member failure and asymmetric partition tests;
- downstream fencing reference implementation.

### Phase C — Exact-candidate live certification

- digest-pinned GitOps deployment;
- one-site power/network loss and leader loss;
- lost-device/credential revocation;
- clean replacement member and clean-room restore;
- secret/ESO generation and namespace boundary;
- telemetry-sink outage and secret-redaction scan;
- seven-day soak and independent reviewer sign-off.

## 8. Release decision rule

The structural validator always runs in pull requests:

```bash
node tools/validate-production-gates.mjs
```

The release job and human go/no-go review run:

```bash
node tools/validate-production-gates.mjs --require-pass
```

`--require-pass` fails when any required row is not `passed`, any row is `failed`, evidence is missing, a required route class has no tests, or an accepted risk is malformed. The command is deliberately simple and dependency-free so it can run in CI, a clean recovery host, or an evidence-review environment.

## 9. Ownership and sign-off

Implementation authors may supply evidence but may not be the only reviewer. The final bundle records:

- exact source and GitOps commits;
- image digests and verified provenance;
- CI run and artifact URLs;
- test environment and time window;
- live synthetic request/trace identifiers;
- backup object/version and restore report;
- failure/partition/revocation reports;
- independent security/reliability reviewer;
- approved cohort, scope, quotas, and next review date.
