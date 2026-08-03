# Fiducia Production Gate Evidence Bundle Template

Copy this document for each exact release candidate. Do not overwrite an earlier decision bundle. A link without an immutable candidate identifier or a test artifact without a declared environment is not sufficient evidence.

## 1. Candidate identity

| Field | Value |
|---|---|
| Decision ID | |
| Decision date/time in UTC | |
| Source commit | |
| Monorepo gitlink/submodule commit set | |
| GitOps/configuration commit | |
| Image digests | |
| SBOM/provenance/signature references | |
| Deployment environment and cells/sites | |
| Approved capabilities | |
| Explicit exclusions | |
| Approved tenant IDs / design partners | |
| Approved data classes | |
| Per-tenant quotas | |
| Decision owner | |
| Independent reviewer | |
| Incident commander/on-call owner | |
| Next review/expiry | |

## 2. Build and configuration evidence

- [ ] Dependencies and generated contracts are pinned to immutable full commits.
- [ ] Lockfiles are enforced and advisory/vulnerability policy passed.
- [ ] Nix, native, and container paths meet the declared parity requirement.
- [ ] OCI SBOM, provenance, signature, and subject/digest verification passed.
- [ ] Every rendered production overlay passed schema and policy validation.
- [ ] Running pod/container `imageID` values match the approved digests.
- [ ] Live GitOps drift report is clean or every emergency difference is documented and reconciled.
- [ ] Stateful rollout changed one healthy follower at a time and the leader last.

Evidence:

- CI runs:
- Attestations:
- Render/policy output:
- Runtime digest inventory:
- Drift report:

## 3. Service contract measurements

Complete the table from the exact observation interval. Do not substitute a dashboard screenshot for the query, source, time range, and exported result.

| SLO | Required target | Query/source | Time range | Observed | Pass/Fail | Evidence |
|---|---:|---|---|---:|---|---|
| `SLO-AVAIL-01` public availability | 99.5% / 28d candidate objective | | | | | |
| `SLO-SUCCESS-01` eligible operation success | 99.9% / 28d candidate objective | | | | | |
| `SLO-READ-01` linearizable read latency | p95 ≤ 500 ms; p99 ≤ 2 s | | | | | |
| `SLO-WRITE-01` committed write latency | p95 ≤ 1.5 s; p99 ≤ 4 s | | | | | |
| `SLO-RENEW-01` eligible renew success | 99.9% | | | | | |
| `SLO-FAILOVER-01` one-site/leader recovery | p95 ≤ 15 s; no event > 30 s | | | | | |
| `SLO-REVOKE-01` credential revocation | p99 ≤ 60 s | | | | | |
| `SLO-SECRET-01` encrypted secret read | p95 ≤ 1 s; p99 ≤ 3 s | | | | | |
| `SLO-WATCH-01` watch reconnect/reconcile | p95 ≤ 15 s | | | | | |
| `SLO-SUPPORT-01` acknowledgement | Sev-0 ≤ 15 m; Sev-1 ≤ 60 m | | | | | |

Error-budget state and enrollment consequence:

## 4. Safety matrix execution

For every row in `production-safety-release-gate.json`, update the canonical JSON status and evidence list. Summarize here by class; the JSON remains the source of truth.

| Class | Required IDs | Passed | Failed | Pending | Evidence index |
|---|---|---:|---:|---:|---|
| Identity/tenant/edge/browser | `AUTH-001..008` | | | | |
| KV/secrets/bootstrap | `KV-001..005` | | | | |
| Consensus/fencing/readiness | `RAFT-001..005` | | | | |
| Restore/rollback | `RESTORE-001..002` | | | | |
| Observability failure/redaction | `OBS-001` | | | | |
| Build/artifact/configuration | `BUILD-001..003` | | | | |
| Device/incident operations | `OPS-001..002` | | | | |

Required strict validation:

```bash
node tools/validate-production-gates.mjs --require-pass
```

Command output / CI run:

## 5. Cross-tenant and credential-plane proof

Record the two or more synthetic tenant fixtures using opaque IDs only.

- [ ] Organization/project/environment isolation passed for every approved operation.
- [ ] Customer, administrator, internal-service, trusted-edge, introspection, and peer credentials cannot cross planes.
- [ ] Direct-node, forged/duplicated trust header, wrong Supabase issuer/project/audience, stale session/membership, and cross-origin tests fail closed.
- [ ] API-key rotation/revocation reaches every supported ingress within the measured bound.
- [ ] Browser storage, cache, console, network, screenshots, and CI artifacts contain no one-time key plaintext.

Evidence:

## 6. Secret-delivery and redaction proof

- [ ] Production plaintext KV writes and alternate bypass shapes are denied before commit.
- [ ] Prefix/path/encoding corpus cannot cross tenant/project/environment/namespace policy.
- [ ] TLS/CA trust and rotation pass without insecure-skip behavior.
- [ ] ESO or equivalent delivery reads only the authorized active generation/path.
- [ ] Canary-secret scan is clean across logs, metrics, traces, audit projections, queues, CI, browser output, exports, and support artifacts.
- [ ] Bootstrap/decryption roots are recoverable independently of Fiducia.

Evidence:

## 7. Consensus, fencing, and failure proof

- [ ] Process, pod, member, current-leader, and one-site loss preserve committed state with a healthy quorum.
- [ ] Asymmetric partition produces one committing majority; minority paths fail closed.
- [ ] Ambiguous mutations are not automatically replayed without the supported idempotency contract.
- [ ] A paused former holder using token `N` is rejected after token `N+1` is authoritative and accepted downstream.
- [ ] Readiness excludes missing storage, actor, quorum, identity, policy, or CA trust.
- [ ] A clean replacement member joins with a new identity and fully catches up before another voter changes.

Fault timeline and evidence:

## 8. Backup, clean-room restore, and rollback proof

| Measurement | Target | Observed | Evidence |
|---|---:|---:|---|
| Backup age at incident/test declaration | ≤ 15 minutes candidate target | | |
| One-site/client recovery | ≤ 30 seconds candidate target | | |
| Clean-room restore RTO | ≤ 4 hours candidate target | | |
| Committed-entry loss under healthy-quorum one-site loss | 0 | | |

- [ ] Restore used an external encrypted backup plus independently controlled key custody.
- [ ] Tenant-safe state digest matched representative source state.
- [ ] Revisions/CAS, locks/leases, fencing tokens, credential versions/revocations, indexes, policy, and audit continuity did not regress.
- [ ] Stateful rollback/migration abort did not replace more than one voter or revive stale authority.
- [ ] Synthetic traffic and stale-fencing tests passed before routing a partner workload.

Evidence:

## 9. Operational readiness

- [ ] Primary/backup incident commander and operations/security/communications owners are reachable.
- [ ] Partner workload owners and emergency contacts were verified.
- [ ] Tenant disable, credential revoke-all, ingress disable, mutation disable/read-only, and rollback paths were exercised.
- [ ] Status and partner communication templates were exercised in the required tabletop.
- [ ] Lost-device revocation passed across mesh, SSH, Git/registry, Kubernetes, TLS, SOPS, backup, monitoring, and runtime secrets.
- [ ] Seven-day soak completed with declared traffic and bounded fault injection.
- [ ] No unresolved Sev-0/Sev-1 or unbounded critical issue remains.

Tabletop/soak evidence:

## 10. Exceptions

No exception may cover an automatic no-go invariant.

| Field | Value |
|---|---|
| Failed criterion / affected row | |
| Affected tenants/capabilities | |
| Risk statement and why bounded | |
| Monitoring and alert | |
| Containment / rollback | |
| Owner | |
| Independent reviewer | |
| Expiry date | |
| Remediation issue | |
| Approval evidence | |

## 11. Final decision

Select exactly one:

- [ ] **GO — bounded beta:** only the cohort, scope, sites, data classes, quotas, and window in Section 1.
- [ ] **HOLD:** no new enrollment or capability expansion; current workloads remain within named containment while issues close.
- [ ] **NO-GO / ROLLBACK:** do not begin or expand real-user workloads; contain, disable, or migrate affected workloads.

### Evidence-based rationale

State which evidence changed the decision. Confidence, effort spent, and the absence of a recent incident are not evidence.

### Required follow-up

| Issue | Owner | Priority | Due/expiry | Launch consequence |
|---|---|---:|---|---|
| | | | | |

### Sign-off

| Role | Name | Decision | UTC timestamp | Evidence/comment |
|---|---|---|---|---|
| Decision owner | | | | |
| Independent security/reliability reviewer | | | | |
| Incident/on-call owner | | | | |
| Partner/customer owner | | | | |
| Capacity/usage owner | | | | |
