# Managed Fiducia Beta Incident Runbook

| Field | Value |
|---|---|
| Contract owner | `DEN-1390` |
| Safety-gate owner | `DEN-1391` |
| Applies to | Approved managed-beta tenants and capabilities only |
| Status | Provisional; must be exercised before launch |

This runbook operationalizes the service contract's severity, containment, communication, and evidence requirements. It never authorizes copying customer secrets, credentials, private keys, request bodies, or raw tenant resource names into chat, Linear, GitHub, status updates, screenshots, or incident artifacts.

## 1. Preconditions before any live beta workload

The release decision must name, for the entire approved workload window:

- primary and backup incident commander;
- operations lead and security lead;
- communications lead;
- each partner's workload owner and emergency contact;
- one monitored paging route and one partner-visible status surface;
- the tenant-disable, credential-revoke-all, ingress-disable, mutation-disable, and read-only procedures;
- independently recoverable bootstrap/key custody contacts;
- the exact GitOps rollback commit and last independently restored backup;
- the approved service scope, sites/cells, data classes, quotas, and stop conditions.

A live workload is not authorized when any required role is unreachable or a containment procedure is undocumented.

## 2. Automatic stop and containment conditions

Declare **Sev-0** and stop new enrollment immediately for any suspected:

1. cross-tenant or cross-plane success;
2. secret, raw credential, private-key, or sensitive payload disclosure;
3. stale fencing token accepted by a downstream protected system;
4. authentication, authorization, revocation, peer-trust, or key-path bypass;
5. unrecoverable committed-state loss or restore that rewinds revisions, fencing tokens, lease epochs, key versions, or revocations;
6. unverified/mutable artifact running in production;
7. active compromise of a site, operator, CI, registry, backup, or bootstrap identity;
8. inability to reach an accountable operator or affected partner owner.

The incident commander may disable the affected tenant, credential class, capability, cell, public ingress, all mutation paths, or the entire beta without prior approval. Safety takes priority over availability.

## 3. First 15 minutes

### 3.1 Acknowledge and establish command

1. Acknowledge the page/report in the incident system.
2. Open an incident record with a stable ID and UTC timestamp.
3. Assign incident commander, operations, security, communications, and scribe roles.
4. State the initial severity and the evidence supporting it. When uncertain between severities, choose the higher severity until bounded.
5. Name the affected capability, cell, and tenant set using internal opaque IDs; do not place secret values or raw customer resource names in the record.
6. Freeze deployments and nonessential operator changes.

### 3.2 Preserve evidence safely

Record only bounded, redacted references:

- release source commit, GitOps commit, image digest, and deployment revision;
- request/trace/event IDs;
- node/pod/site IDs and Raft shard/term/index where relevant;
- alert and dashboard URLs with access control;
- backup object/version IDs without credentials or signed URLs;
- exact time range and operator actions.

Do not paste bearer tokens, API keys, cookies, JWTs, certificate/private-key material, secret values, request bodies, environment dumps, `kubectl describe secret`, or unredacted logs.

### 3.3 Contain before diagnosing deeply

Choose the smallest action that reliably bounds harm, but do not preserve availability at the cost of safety:

- revoke/disable the affected tenant or credentials;
- remove one compromised site or identity from mesh, SSH, Git/registry, Kubernetes, TLS, SOPS, backup, and runtime-secret access;
- disable a capability or all mutations while preserving safe reads where proven;
- remove public ingress to an affected cell;
- stop a rollout and pin the last independently verified digest;
- isolate telemetry/export destinations suspected of leakage while retaining bounded local evidence queues;
- fence the partner's downstream workload and require the highest committed token before resuming.

Every emergency live change must be timestamped, peer-reviewed as soon as practical, and reconciled back to Git before normal rollout resumes.

## 4. Severity and communication cadence

| Severity | Examples | Acknowledge | Update cadence | Required decision authority |
|---|---|---:|---:|---|
| **Sev-0** | Isolation/security breach, secret disclosure, stale fencing accepted, unrecoverable state loss, credential authority bypass | 15 minutes | Every 30 minutes until contained | Incident commander and security lead; partner owner for workload validation |
| **Sev-1** | Quorum unavailable, broad approved-capability outage, missed revocation bound, recovery objective at risk | 60 minutes | Every 60 minutes | Incident commander and service owner |
| **Sev-2** | Bounded single-tenant defect or SLO burn with safe workaround | 1 business day | Daily/material change | Service owner |
| **Sev-3** | Documentation, cosmetic, low-risk request | 3 business days | At resolution/milestone | Product/service owner |

Partner/status updates use `docs/operations/managed-beta-communication-templates.md`. Each update states confirmed impact, containment, customer action, and the next update time. It does not speculate or disclose another tenant's identity, exploit details, internal topology secrets, or sensitive evidence.

## 5. Incident-specific containment playbooks

### 5.1 Cross-tenant or authorization anomaly

1. Disable the implicated tenant credentials and the affected route class.
2. Preserve redacted request IDs and authoritative identity/policy versions.
3. Test the same path with two clean synthetic tenants through every ingress.
4. Check customer/admin/internal/peer credential-plane separation and trusted-header stripping.
5. Determine whether any read, mutation, watch, audit, cache, export, or support artifact crossed the boundary.
6. Do not restore the route until the exact candidate passes the corresponding `AUTH-*` or `KV-*` matrix rows and an independent reviewer accepts the evidence.

### 5.2 Suspected secret or credential disclosure

1. Stop the source and destination of disclosure; do not copy the value again for confirmation.
2. Revoke/rotate the credential or secret and all dependent credentials that cannot be proven independent.
3. Purge or access-restrict affected telemetry, CI, browser, support, and export artifacts under the retention/legal process; retain a cryptographic fingerprint or opaque incident marker rather than the value.
4. Run the canary-secret scan across all configured stores and time windows.
5. Verify old credentials fail on every supported ingress within the declared bound.
6. Review whether backup copies or downstream systems require additional rotation.

### 5.3 Quorum, leader, or one-site failure

1. Confirm which members are voters, current leader/term, commit index, follower lag, and whether a healthy majority remains.
2. During planned maintenance or a controlled upgrade, change **one healthy follower at a time and the leader last**.
3. During incident recovery, do not restart, replace, or upgrade a second voter while one is unhealthy or catching up.
4. Remove unhealthy public origins and allow only validated membership-bound routing hints.
5. Treat mutation timeouts as ambiguous; retry only through the documented idempotency contract.
6. Verify partner workloads stop on lost renewal and reject stale fencing tokens downstream.
7. Replace/recover the failed member follower-first with a stable new identity; verify full catch-up before restoring redundancy.

### 5.4 Restore or rollback event

1. Freeze writes unless the incident commander documents a safe quorum-preserving path.
2. Identify the exact backup object/version, encryption-key versions, source commit, and schema/protocol compatibility.
3. Restore into a clean isolated cluster first.
4. Verify tenant-safe state digest, revisions/CAS, locks/leases, fencing tokens, credential versions/revocations, indexes, policy, and audit continuity.
5. Run synthetic reads/writes and stale-fencing tests before routing any partner workload.
6. Never rewind production state merely because an older image starts successfully.

### 5.5 Lost or compromised laptop/site identity

Revoke the identity from every plane, not only the mesh VPN:

- Tailscale/WireGuard or equivalent mesh;
- SSH and device-management access;
- GitHub, container registry, CI/deployment credentials;
- Kubernetes client and service-account material;
- TLS certificates and peer trust;
- SOPS/age/KMS recipients and runtime secret access;
- backup/object-store access;
- monitoring, paging, status, and support systems.

After revocation, probe each old identity and record denials. A replacement joins with a new identity and catches up as one voter; credentials from the lost device are never reused.

## 6. Recovery and validation

Before declaring monitoring/resolved:

- containment remains effective after process restart, cache expiry, leader change, and one-site failure;
- the exact fixed digest is verified at runtime and matches the reviewed GitOps commit;
- affected `production-safety-release-gate.json` rows have new evidence and no unexplained regression;
- cross-tenant negative tests, credential revocation, stale-fencing rejection, and secret-redaction scans pass where relevant;
- committed state and audit continuity are reconciled;
- the affected partner validates its downstream workload from a clean credential/session;
- alerts, external probes, backup age, quorum, disk, network, and telemetry paths are healthy for the declared observation window.

`Monitoring` is not `Resolved`. Resolution requires a stable observation period chosen for the failure mode and approval from the incident commander plus the responsible security/reliability owner.

## 7. Post-incident review

Sev-0 and Sev-1 incidents receive a redacted review within five business days. The review includes:

- impact and timeline in UTC;
- how detection occurred and where it was delayed;
- technical and organizational contributing conditions;
- why existing tests, alerts, policies, or reviews did not prevent/detect sooner;
- containment and recovery decisions, including rejected alternatives;
- measured SLO/error-budget, RPO/RTO, revocation, and support effects;
- evidence links and affected gate rows;
- remediation issues with owners, priority, and expiry dates;
- whether to expand, hold, or stop the beta.

Blameless analysis does not mean ownerless remediation.

## 8. Required pre-launch tabletop

Exercise at least this combined scenario before DEN-1390 can close:

1. The current leader's site loses power during a credential rotation.
2. A stale positive introspection cache continues accepting the old key near the declared overlap bound.
3. The old workload resumes after partition healing and attempts a write with an older fencing token.
4. The telemetry gateway becomes unavailable while evidence is being collected.
5. The team must declare severity, contain the tenant/capability, communicate with the partner, prove downstream stale-token rejection, verify revocation across all ingresses, restore telemetry safely, and make a go/hold/stop decision.

Record timestamps, role assignments, decisions, missed steps, communication artifacts, and resulting Linear issues. Do not mark the exercise complete merely because participants discussed what they would do.
