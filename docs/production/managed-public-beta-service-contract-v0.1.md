# Managed Fiducia Public Beta Service Contract v0.1

| Field | Value |
|---|---|
| Status | **Provisional launch candidate — not yet approved** |
| Version | `0.1.0` |
| Linear owner | `DEN-1390` |
| Applies to | The explicitly enrolled 3–5 design-partner cohort only |
| Effective date | Not before DEN-1390 and DEN-1391 receive independent approval |
| Review cadence | Weekly during the beta; after every Sev-0/Sev-1; before any cohort expansion |

> This document defines **internal engineering launch objectives and bounded beta behavior, not a contractual SLA**. No customer-facing availability, durability, geography, compliance, or recovery promise may exceed measured evidence from the exact deployed release candidate.

## 1. Product boundary

### 1.1 Included beta capabilities

The first managed beta may expose only the following capabilities after their gate rows have passed:

1. **Multi-key fenced locks** — atomic union-of-key acquisition, bounded waiting, renewable TTL, FIFO conflict ordering, cancellation, release, and monotonically increasing fencing tokens.
2. **Counting semaphores** — tenant-scoped bounded concurrency with leases and fencing.
3. **Renewable leases and leader election** — named leadership with TTL, renew/release, current-leader reads, watches, and fencing tokens.
4. **Idempotency records** — first-claim and exact-response replay for supported mutations; the public load balancer retains an eligible response for the configured replay window.
5. **Encrypted, revisioned KV and watches** — linearizable reads/writes, compare-and-set using revisions, encryption before replication, and reconnectable SSE notifications.
6. **Rate limiting** — atomic tenant-and-key admission decisions for protecting services and quotas. Rate-limit counters are not the customer billing ledger.
7. **Authentication and credential lifecycle required to use those capabilities** — human Supabase sessions for the dashboard/control plane; scoped Fiducia API keys or Fiducia-issued JWTs for the data plane; create, rotate, revoke, and bounded positive-cache overlap.

### 1.2 Explicitly excluded from the initial beta

The following may exist in source code but are not part of the launch promise until separately approved with SDK, support, quota, failure-mode, and recovery evidence:

- cron/schedules and customer function execution;
- service discovery, counters, barriers, tasks, effects, handoffs, decisions, budgets, and claims;
- dynamic provider credentials and automated secret rotation;
- automated billing or contractual usage invoices;
- compliance attestations, data residency commitments, or regulated-workload support;
- a guarantee of uninterrupted service after two concurrent site failures;
- a claim that synthetic `aws`, `gcp`, `azure`, or other provider labels mean the workload actually runs in those providers.

Adding a capability requires a versioned contract change, corresponding release-gate rows, SDK conformance, quota coverage, support readiness, and an approved go/no-go decision.

## 2. Deployment and availability truth

The bounded beta may initially run on three separately operated K3s clusters hosted on three physical laptops/sites. Each Fiducia shard uses three voting replicas distributed one per declared failure domain. The intended safety envelope is:

- loss of any **one** site may preserve quorum and committed reads/writes;
- while one site is unavailable, the service has no additional site-failure tolerance;
- planned stateful changes replace or upgrade one healthy follower at a time and the leader last;
- the public edge routes only to ready regional load balancers; load balancers route to current shard leaders and do not hold consensus state;
- synthetic provider names are placement/test labels only and must never be represented as actual AWS, GCP, Azure, or other cloud-provider infrastructure;
- the temporary substrate is eligible only for the data classes in Section 3 and only after clean-room restore, one-site failure, lost-device revocation, and seven-day soak evidence has passed.

Broad general availability requires a separate contract and a reviewed migration to production-grade cloud or colocated failure domains.

## 3. Tenant and data boundaries

### 3.1 Required isolation hierarchy

Every request must be bound to a verified identity and the narrowest implemented resource boundary:

`organization → project → environment → primitive/resource path → operation/scope`

A missing or invalid boundary fails closed. Customer, administrator, internal-service, and Raft-peer credentials are distinct and cannot be substituted across planes.

### 3.2 Permitted beta data

Design partners may store or coordinate:

- ordinary internal operational metadata;
- identifiers and state used for non-regulated background jobs;
- application credentials and configuration classified by the customer as confidential, provided the customer accepts the beta boundary and uses the supported encrypted-secret path;
- test, staging, shadow, and bounded production coordination state approved in the partner workload record.

### 3.3 Prohibited beta data and workloads

The initial beta must not be used for:

- payment-card data, protected health information, government-classified data, export-controlled technical data, private cryptographic root keys, or other regulated/special-category data;
- custody of funds, signing authority, irreversible safety-critical control, life-support, emergency-response dispatch, or legal-record systems without an independently reviewed exception;
- workloads whose loss, duplication, or temporary unavailability could cause unbounded financial, physical, legal, or privacy harm;
- secrets whose only recovery copy or decryption root is itself stored solely in Fiducia.

## 4. Consistency and operation semantics

### 4.1 General request semantics

- Successful strongly consistent writes are acknowledged only after the owning shard commits the command to a Raft quorum.
- Leader-only reads are linearizable. A follower does not invent a successful response when it is not authoritative.
- An explicit `NotLeader` response is safe for the load balancer to retry against a validated healthy member. A lost response to a mutation is **ambiguous** and is not automatically replayed unless the caller supplied the supported idempotency contract.
- Client timeouts do not prove failure. A timed-out mutation may have committed; clients retry with the same idempotency key or reconcile with a read.
- Fiducia provides coordination and fencing information. It cannot make an external database, queue, payment processor, or file store safe unless that downstream system records and rejects stale fencing tokens or uses an equivalent transactional guard.
- Every client uses bounded exponential backoff with jitter and respects `Retry-After` where supplied.

### 4.2 Locks and semaphores

- A multi-key lock conflicts with any holder or queued reservation that overlaps at least one key. Acquisition is all-or-nothing; no partial key set is granted.
- A grant carries an absolute lease expiry and a monotonically increasing fencing token for its coordination domain.
- Renewal requires the current holder identity and fencing token. Renewal extends expiry but does not reset the token.
- A client that cannot confirm renewal before its local safety deadline must stop protected work. The recommended deadline is no later than one third of the configured TTL remaining.
- Release and cancellation are retry-safe only through the documented request identity/token rules. Customers must not infer ownership from a network timeout.

### 4.3 Leader election and renewable leases

- At most one current lease is authoritative for a named election in committed state.
- Leadership transfers or expiries issue a newer fencing token. A former leader must stop work when renewal fails, times out beyond its safety deadline, or reports `not_leader`.
- Customers pass the fencing token to the downstream protected system and reject writes using a token lower than the highest accepted token.

### 4.4 Encrypted KV and watches

- Production writes use the encrypted path. The production policy forbids or explicitly deny-lists `plaintext:true` writes.
- KV revisions are monotonic within the authoritative store and are used for compare-and-set. Restore and rollback procedures must not rewind visible revisions, credential versions, lease epochs, or fencing state.
- Watch streams are reconnectable notifications, not a durable exactly-once message queue. A client treats notifications as at-least-once/invalidation signals, resumes from a supported revision when possible, and performs a linearizable read after a gap or reconnect.
- Secret values must not be copied into logs, traces, metrics labels, URLs, Linear, Git, CI output, screenshots, browser persistence, exports, or support artifacts.

### 4.5 API-key rotation and revocation

- API keys are scoped to an organization and allowed data-plane scopes. They cannot mint or inherit administrator-dashboard authority.
- Rotation advances an authoritative credential version immediately. A previously cached positive decision may remain valid only for the declared overlap window.
- The initial beta launch target is a **maximum 60-second positive introspection cache**, matching the current default. A stricter environment may reduce it.
- Revocation is complete only when authoritative state is revoked and every supported edge/load-balancer path rejects the key within the revocation SLO.

## 5. Launch SLOs and indicators

These are candidate engineering thresholds. DEN-946 and DEN-1392 must measure them against the exact release digest. A threshold that is not met must be revised before enrollment or causes a no-go; it must not be silently advertised as achieved.

Eligible requests exclude customer-originated `4xx` caused by invalid authentication, authorization, body, quota, conflict, or rate-limit input. Server `5xx`, transport failures after reaching Fiducia, readiness routing failures, timeouts, and ambiguous upstream outcomes count against the relevant SLI.

| ID | Objective | Queryable indicator and calculation | Alert / stop threshold | Owner | Review |
|---|---|---|---|---|---|
| `SLO-AVAIL-01` | **99.5%** public API availability over a rolling 28 days for each approved cell | External synthetic success ratio: `successful eligible probes / all eligible probes`, from at least two independent probe locations, recorded as `fiducia:sli:public_availability_ratio` | Page when 1-hour burn rate > 14.4× or 6-hour burn rate > 6×; freeze enrollment at 100% error-budget consumption | Reliability owner | Daily; weekly partner review |
| `SLO-SUCCESS-01` | **99.9%** successful eligible data-plane operations over 28 days | `eligible 2xx/expected conflict outcomes / eligible requests` from canonical OTel HTTP metrics, recorded as `fiducia:sli:data_plane_success_ratio` | Page on 5-minute ratio < 99%; investigate any 1-hour ratio < 99.5% | Data-plane owner | Daily |
| `SLO-READ-01` | Linearizable read latency: p95 ≤ **500 ms**, p99 ≤ **2 s** | Histograms for completed KV/lock/election/status reads measured at the public LB, recorded as `fiducia:sli:linearizable_read_seconds` | Page when p99 > 4 s for 10 minutes; hold release when 28-day p95/p99 miss target | Data-plane owner | Daily and per release |
| `SLO-WRITE-01` | Committed write latency: p95 ≤ **1.5 s**, p99 ≤ **4 s** | Histograms for committed acquire/renew/release, election, KV/CAS, idempotency, and rate-limit operations at the public LB, recorded as `fiducia:sli:committed_write_seconds` | Page when p99 > 8 s for 10 minutes; hold release when soak p95/p99 miss target | Data-plane owner | Daily and per release |
| `SLO-RENEW-01` | **99.9%** successful eligible lock/lease renewals submitted with at least one third of TTL remaining | `successful committed renewals / eligible renewals`, excluding invalid or stale-token attempts, recorded as `fiducia:sli:renew_success_ratio` | Page on 5-minute success < 99%; stop affected customer mutation if renew safety margin is exhausted | Data-plane + partner workload owner | Continuous |
| `SLO-FAILOVER-01` | One-site or current-leader loss: p95 client recovery ≤ **15 s** and no observed event > **30 s** during certification | Fault-injection event timestamp to first sustained successful committed request; recorded in `fiducia:sli:failover_recovery_seconds` and linked to the exact candidate | Any stale mutation, data loss, or event > 30 s is automatic no-go; page after 15 s in production | Reliability owner | Every drill and incident |
| `SLO-REVOKE-01` | Revoked/rotated API key rejected on every supported ingress at p99 ≤ **60 s** | Authoritative version timestamp to last successful old-credential probe, recorded as `fiducia:sli:credential_revocation_seconds` | Any acceptance after 60 s is release-blocking; emergency containment pages immediately | Identity owner | Every release and rotation drill |
| `SLO-SECRET-01` | Encrypted secret/KV read p95 ≤ **1 s**, p99 ≤ **3 s** | End-to-end customer/ESO synthetic read histogram without secret content, recorded as `fiducia:sli:secret_read_seconds` | Page when p99 > 6 s for 10 minutes or delivery is stale beyond declared generation | Secret-delivery owner | Daily |
| `SLO-WATCH-01` | p95 reconnect-to-current-state ≤ **15 s** after a planned LB/node interruption | Synthetic watcher interruption to successful reconnect plus linearizable reconciliation, recorded as `fiducia:sli:watch_recovery_seconds` | Alert when p95 > 30 s; customer must fall back to bounded polling/reconciliation | Client/SDK owner | Per release |
| `SLO-SUPPORT-01` | Sev-0 acknowledgement ≤ **15 min**; Sev-1 ≤ **60 min** while beta workloads are live | Incident system timestamps: `acknowledged_at - reported_at`; reviewed with partner roster and on-call coverage | Missed Sev-0 acknowledgement is automatic enrollment freeze and incident-review trigger | Incident commander/on-call owner | Every incident; weekly |

### 5.1 Error-budget policy

For `SLO-AVAIL-01`, 99.5% over 28 days permits approximately 3 hours 22 minutes of unavailable time. The following controls apply to the most constrained SLO/error budget:

- **≥ 50% consumed in 7 days:** stop nonessential rollout and open a reliability review.
- **≥ 75% consumed in 28 days:** freeze new tenant enrollment and capability expansion.
- **100% consumed or any automatic no-go invariant breached:** stop enrollment, contain affected mutation paths, and require a written hold/rollback decision.
- Planned maintenance counts against the beta availability indicator unless the customer contract approved before enrollment explicitly says otherwise.

## 6. Recovery objectives

These are engineering launch targets and require measured clean-room evidence:

| Scenario | RPO target | RTO target | Required proof |
|---|---:|---:|---|
| Loss of one site while a healthy quorum remains | **0 committed Raft entries** | First sustained successful write within **30 s** | Repeated leader and follower power/network-loss drills; downstream stale-fencing rejection |
| Loss/replacement of one member | **0 committed Raft entries** | Member replaced and caught up within **2 h** | New-identity replacement drill; no simultaneous second-voter replacement |
| Clean-room rebuild from external encrypted backup | Backup age ≤ **15 min** at incident declaration | Supported beta surface restored within **4 h** | Restore into a clean cluster with revisions, leases, fencing tokens, key versions, revocations, and tenant boundaries verified |
| Customer credential compromise | No new accepted request after the revocation bound | Emergency disable initiated within **15 min** of confirmed Sev-0 report | Revoke-all/key-rotation drill across all ingress paths |

Fiducia bootstrap and decryption roots must be recoverable independently of Fiducia. A backup upload without a successful restore does not count as evidence.

## 7. Customer responsibilities

Each design partner must:

- use TLS endpoints and protect API keys as production secrets;
- use a separate organization/project/environment and least-privilege credential for each workload boundary;
- supply idempotency keys for retryable mutations and reconcile ambiguous outcomes;
- renew leases early, stop protected work on renewal uncertainty, and enforce fencing tokens in the downstream stateful system;
- treat watches as reconnectable notifications and reconcile after gaps;
- remain within approved quotas and contact support before planned load changes;
- maintain an independent recovery path for application credentials and data whose loss would prevent the application from starting;
- provide an accountable workload owner and emergency contact.

## 8. Support and incident model

### 8.1 Severity matrix

| Severity | Definition | Initial acknowledgement | Communication cadence | Default containment authority |
|---|---|---:|---:|---|
| **Sev-0 — Critical safety/security** | Cross-tenant access; secret disclosure; stale fencing accepted downstream; unrecoverable committed-state loss; credential authority bypass; active compromise | 15 minutes | Every 30 minutes until contained, then agreed cadence | Incident commander may disable tenants, ingress, mutation paths, credentials, or the whole beta without prior approval |
| **Sev-1 — Major outage/degradation** | Quorum unavailable; one or more approved capabilities broadly failing; recovery objective at risk; sustained severe latency; revocation bound missed without known compromise | 60 minutes | Every 60 minutes | Incident commander may stop rollout, fail over, disable affected capability, or place service read-only |
| **Sev-2 — Limited impact** | Single-tenant defect with bounded workaround; partial dashboard/SDK failure; SLO burn without immediate safety risk | 1 business day | Daily or on material change | Service owner coordinates workaround and scheduled fix |
| **Sev-3 — Minor/request** | Documentation defect, cosmetic issue, feature request, low-risk operational question | 3 business days | At resolution or agreed milestone | Product/service owner |

### 8.2 Required roles

Before the first live workload, the roster names:

- **Incident commander:** owns severity, containment, decision log, and transition to recovery.
- **Operations lead:** executes infrastructure/data-plane actions and preserves evidence.
- **Security lead:** owns credential, tenant, secret, and compromise decisions.
- **Communications lead:** updates affected partners and the status page without exposing sensitive details.
- **Partner workload owner:** can stop or fence the customer workload and validate recovery.

One person may hold multiple roles in the small beta, but incident commander and hands-on operator should be separated whenever possible.

### 8.3 Communication and status

- A public or partner-visible status surface reports investigating, identified, monitoring, and resolved states.
- Affected partners receive the first notice after impact is confirmed and no later than the severity acknowledgement target.
- Updates state known impact, containment, customer action, and next-update time; they do not speculate or reveal secrets, exploit details, tenant identifiers, or internal credentials.
- Sev-0 and Sev-1 incidents receive a redacted written review within five business days, with remediation owners and expiry dates.

## 9. Maintenance and change policy

- Planned customer-impacting maintenance normally receives at least **72 hours** notice and names expected impact, rollback, and customer action.
- Stateful maintenance is quorum checked, follower first, one voter at a time, leader last. A Kubernetes rolling-update setting is not sufficient evidence.
- The exact release candidate is built once, identified by immutable image digest, verified for provenance/SBOM/policy, and promoted through GitOps. Production pods do not build from source or follow mutable branches/tags.
- Emergency security maintenance may occur without advance notice; the incident process and partner communication still apply.
- Live edits are prohibited except an explicitly logged emergency containment action. Every emergency live change is reconciled back to Git and reviewed before normal rollout resumes.

## 10. Retention, deletion, and offboarding targets

The beta enrollment record must name the final configured retention values. The default launch targets are:

- active coordination and encrypted KV state: retained until customer deletion or account termination;
- deleted live values: removed from the serving state within 24 hours after an authorized deletion completes;
- encrypted backup copies of deleted values: age out within 35 days unless a documented security incident or legal hold applies;
- ordinary operational logs/traces: 14 days;
- security and high-value audit metadata: 90 days, without secret values or raw credentials;
- offboarding: revoke credentials immediately, stop or expire active leases/watches safely, export non-secret metadata/audit where supported, and schedule deletion under the above policy.

No retention statement is customer-facing until backup lifecycle, log storage, export, and deletion tests verify it in the deployed environment.

## 11. Launch exceptions

An exception is valid only when it records all of the following:

- failed criterion and affected tenant/capability;
- bounded risk statement and why no automatic no-go invariant is affected;
- named owner and independent reviewer;
- monitoring/alert and containment/rollback procedure;
- expiry date and remediation Linear issue;
- explicit approval in the release decision record.

Cross-tenant access, secret disclosure, stale fencing accepted downstream, unrecoverable committed-state loss, auth bypass, mutable production artifacts, missing accountable operator, or inability to restore are **not waivable** for the beta.

## 12. Approval and evidence

This contract may move from provisional to approved only when:

1. the machine-readable DEN-1391 matrix covers every public and trusted-hop route;
2. each required test row is `passed` with CI or live-environment evidence tied to the exact release digest;
3. DEN-946 and DEN-1392 contain measured failover, backup/restore, revocation, and soak reports;
4. the customer-facing documentation matches this boundary and contains no infrastructure/compliance overclaim;
5. a reviewer other than the implementation author signs the go/no-go record;
6. the final decision names the approved cohort, capabilities, data classes, sites/cells, quotas, and next review date.

Run the structural check with:

```bash
node tools/validate-production-gates.mjs
```

Run the evidence-complete release check with:

```bash
node tools/validate-production-gates.mjs --require-pass
```
