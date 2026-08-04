# Managed Fiducia Beta Communication Templates

Use these templates for partner-visible communication. Replace bracketed fields with bounded facts. Never include customer secrets, credentials, request bodies, internal authentication headers, private hostnames/IPs, raw tenant resource names, or unredacted logs.

## 1. Initial incident notice

**Title:** Investigating Fiducia [capability/cell] impact — [incident ID]

**Status:** Investigating  
**Started:** [UTC timestamp]  
**Affected scope:** [approved capability/cell and bounded tenant set]  
**Confirmed impact:** [what customers can or cannot safely do]  
**Containment:** [credential/capability/cell/mutation path disabled or other action]  
**Customer action:** [stop protected work, preserve idempotency key, rotate credential, no action, etc.]  
**Next update:** [UTC timestamp no later than severity cadence]

We are investigating [one-sentence confirmed condition]. We have [contained/not yet contained] the affected path. Do not [specific unsafe action, if any]. We will update by the time above even when there is no material change.

## 2. Identified / containment update

**Title:** Fiducia [incident ID] — cause identified and containment active

**Status:** Identified  
**Current impact:** [bounded description]  
**Cause class:** [authorization / credential revocation / quorum / routing / storage / release / telemetry / other, without exploit detail]  
**Containment:** [what is disabled/fenced/revoked]  
**Recovery work:** [high-level safe action]  
**Customer action:** [specific action or none]  
**Next update:** [UTC timestamp]

We have identified [bounded cause statement]. [Affected operations] remain [disabled/degraded] while we verify [cross-tenant isolation, revocation, stale-fencing rejection, restore integrity, etc.]. We will not resume the affected path until the required safety checks pass.

## 3. Monitoring update

**Title:** Fiducia [incident ID] — recovery complete, monitoring

**Status:** Monitoring  
**Recovered at:** [UTC timestamp]  
**Validated:** [tests/evidence categories, not secret details]  
**Residual limitation:** [if any]  
**Customer action:** [credential replacement/restart/reconciliation or none]  
**Next update:** [UTC timestamp]

Service has been restored using release digest [short safe identifier] and we have verified [bounded list]. We are monitoring for [specific recurrence indicators]. `Monitoring` does not yet mean the incident is resolved.

## 4. Resolution notice

**Title:** Resolved — Fiducia [incident ID]

**Resolved:** [UTC timestamp]  
**Impact window:** [start–end UTC]  
**Affected scope:** [capability/cell/tenant set]  
**Customer action:** [required action or none]  
**Review:** [date/window for redacted review where applicable]

The incident is resolved after [bounded recovery summary] and [observation period]. We verified [relevant isolation/revocation/fencing/restore checks]. A redacted review will document impact, contributing conditions, and remediation without exposing customer-sensitive evidence.

## 5. Planned maintenance notice

**Title:** Planned Fiducia maintenance — [capability/cell] — [date]

**Window:** [start–end UTC]  
**Expected impact:** [none / brief reconnects / bounded unavailability / mutations paused]  
**Affected scope:** [capability/cell/tenants]  
**Customer preparation:** [renewal/idempotency/reconciliation guidance]  
**Rollback trigger:** [safe high-level condition]  
**Status updates:** [status location]

The maintenance will promote immutable release digest [safe identifier] through the reviewed GitOps path. Stateful members will be changed one healthy follower at a time and the leader last. We will stop or roll back if quorum, lag, readiness, safety tests, or the declared SLO envelope fails.

## 6. Emergency maintenance notice

**Title:** Emergency Fiducia maintenance — [incident/change ID]

**Started:** [UTC timestamp]  
**Reason:** [bounded security/reliability reason]  
**Current impact:** [description]  
**Customer action:** [description or none]  
**Next update:** [UTC timestamp]

We are applying an emergency containment/change to protect [tenant isolation / credentials / committed state / service safety]. Advance notice was not possible. All live changes are being recorded and will be reconciled to Git before normal rollout resumes.

## 7. Internal handoff block

Copy this block between incident commanders/operators; keep it in an access-controlled incident system:

```text
Incident ID:
Severity / why:
Current incident commander:
Operations / security / communications leads:
Affected tenants/capabilities/cells (opaque IDs only):
Start time / last update / next update (UTC):
Current containment:
Current release source commit / GitOps commit / image digest:
Highest confirmed Raft term/index/revision/fencing or credential version (safe metadata only):
Evidence links:
Open hypotheses (clearly labeled):
Actions in progress, owner, stop condition:
Do-not-do constraints:
Partner owner/contact status:
Next decision point:
```

## 8. Language rules

Use:

- “confirmed,” “observed,” “under investigation,” and “not yet known” distinctly;
- absolute UTC timestamps;
- bounded capability/cell/tenant descriptions;
- the next promised update time;
- explicit customer safety instructions.

Avoid:

- unsupported claims such as “no data was affected” before evidence exists;
- “exactly once” when the actual contract is idempotent retry plus downstream fencing;
- representing synthetic provider labels as real AWS/GCP/Azure hosting;
- calling engineering SLO targets contractual SLAs;
- pasting raw diagnostic material into partner-visible messages.
