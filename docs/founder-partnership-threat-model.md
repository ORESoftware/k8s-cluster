# Founder Partnership Control Plane — threat model and invariants

**Status:** Discovery draft  
**Linear:** DEN-158, DEN-175  
**Related architecture:** `docs/founder-partnership-control-plane.md`

## Purpose

This document defines the security, safety, and availability properties for a
Fiducia capability that gives founders joint technical control without creating
permanent 2-of-2 deadlock.

The control plane protects actions performed through connected systems. It does
not determine legal ownership, death, incapacity, fiduciary authority, or the
validity of corporate acts. Those facts enter the system as externally attested
claims whose legal sufficiency must be established outside this software.

## Security objective

> No participant can unilaterally dispossess another protected stakeholder, and
> no participant can indefinitely prevent bounded business continuity outside a
> transparent process agreed while all protected stakeholders were active.

The two halves are intentionally in tension:

- **Safety:** prevent unilateral takeover, dilution, self-dealing, and policy
  weakening.
- **Liveness:** permit bounded routine and defensive operations during loss,
  incapacity, death, exit, nonparticipation, or infrastructure failure.

A design that satisfies only safety becomes a mutual-hostage system. A design
that satisfies only liveness becomes a unilateral-takeover system.

## Protected assets

| Asset | Security concern |
| --- | --- |
| Governance policy | A participant must not weaken quorum, notice, cooldown, or self-dealing restrictions unilaterally. |
| Participant registry | Roles and credential bindings must come from tenant-controlled state, not self-asserted approval fields. |
| Proposal | Approvals must bind to the exact action, parameters, policy version, state, nonce, and expiry. |
| Approval | It must be authentic, non-replayable, attributable, and valid for the participant's role and credential at approval time. |
| Continuity state | A false death, incapacity, abandonment, or emergency claim must not grant irreversible power. |
| Execution credential | A founder must not retain a raw bypass credential that ignores Fiducia approval. |
| Fencing authority | A stale executor must not complete an action after lease loss or failover. |
| Audit history | Participants must not erase, reorder, or silently fork material governance evidence. |
| External account state | Off-platform owner, IAM, DNS, recovery, or branch-policy changes must be detected. |
| Economic interests | Continuity authority must not be usable to transfer value to the actor invoking it. |

## Actors

### Protected participants

- **Founder A / Founder B:** economically or constitutionally protected
  stakeholders. Each has independent credentials.
- **Guardian or continuity fiduciary:** independently controlled role with narrow,
  predefined recovery and continuity authority.
- **Estate representative or successor:** activated only through an externally
  valid succession procedure.

### Operational actors

- **Operator:** performs bounded routine actions but has no constitutional power.
- **Fiducia executor:** service identity that uses the external provider
  credential after policy authorization.
- **Policy administrator:** proposes policy changes but cannot bypass the current
  policy's approval rule.

### External actors

- **Identity provider / HSM / KMS:** authenticates participants or protects
  execution keys.
- **Connected provider:** GitHub, cloud IAM, DNS/registrar, identity, finance,
  cap-table, or app-store service.
- **Attestor:** medical, fiduciary, estate, corporate, or legal actor that supplies
  evidence; Fiducia records the assertion but does not declare its legal truth.
- **Provider support staff:** may possess recovery powers outside normal APIs and
  therefore represent a bypass path.

### Adversaries

- A malicious or compromised founder.
- A malicious or compromised guardian.
- Founder-plus-guardian collusion.
- An external attacker holding one or more credentials.
- A compromised Fiducia executor or control-plane node.
- A malicious provider administrator or support channel.
- A network adversary causing delay, replay, partition, or reordering.

## Trust boundaries

1. **Participant device → authentication verifier**
   - WebAuthn or equivalent assertions cross an untrusted network.
   - The verifier must validate challenge, origin/RP ID, signature, credential,
     counter/backup semantics, and tenant binding.
2. **Authentication verifier → participant registry**
   - A role claimed in an approval is untrusted until matched to the registry.
3. **Proposal service → replicated policy state**
   - Reads must not mint authority or mutate state outside the replicated log.
4. **Policy evaluator → executor**
   - The executor receives a short-lived, proposal-bound capability, not a broad
     reusable approval result.
5. **Executor → connected provider**
   - Provider calls can time out after taking effect. Idempotency, reconciliation,
     and an `unknown` receipt state are required.
6. **Control plane → HSM/KMS**
   - Signing or provider credentials should be non-exportable where feasible.
7. **Fiducia → external attestor/legal process**
   - Evidence integrity can be verified technically; legal sufficiency cannot.
8. **Provider API → provider support/recovery plane**
   - API enforcement may be bypassed through support. Drift detection and
     operational response are required.

## Explicit trust assumptions

- At least one required authorization domain remains honest for every protected
  action. A policy requiring Founder A + Founder B cannot protect against both
  founders colluding.
- The guardian is administratively, financially, and technically independent
  enough that its compromise is not equivalent to compromise of a founder.
- Fiducia controls or can disable the relevant provider execution credential.
  Otherwise enforcement is advisory.
- Time used for cooldown and expiry is derived from a trusted monotonic or
  consensus-backed source; one process's wall clock is not authoritative.
- The replicated policy/continuity state and fencing high-water survive restart
  without regression.
- External providers expose enough state to detect material drift, or the product
  clearly documents the unenforceable gap.
- Legal documents identify the relationship between software roles and corporate
  authority. The software does not invent that authority.

## Safety invariants

The following are intended to become executable predicates or model-checking
properties.

### S1 — exact proposal binding

An approval counts only when it signs the canonical hash of the same tenant,
proposal ID, action kind, action class, canonical parameters, policy ID, policy
version, policy hash, proposer, continuity state, nonce, creation time, and
expiry used by the executor.

Changing any committed field invalidates every prior approval.

### S2 — authenticated registry binding

An approval counts only when:

- the participant exists in the tenant-controlled registry;
- the participant is active at approval time;
- the claimed role is assigned to that participant;
- the credential is registered to that participant;
- the cryptographic assertion verifies for the proposal challenge.

Approval payloads cannot grant their own roles.

### S3 — unique-human quorum

Multiple approvals from the same participant count once, regardless of the
number of credentials, roles, retries, or duplicate messages. A policy may
explicitly require both distinct participant IDs and role coverage.

### S4 — non-weakening policy replacement

A replacement is accepted only when it preserves or strengthens every current
state rule, including:

- minimum approvals;
- required participant IDs and roles;
- notice recipients;
- cooldown and challenge windows;
- maximum delegated-authority duration;
- monetary limits;
- allowed/denied action sets;
- ownership, policy-weakening, audit-deletion, and related-party restrictions.

The replacement must itself be approved under the current effective policy, not
under the proposed weaker policy.

### S5 — constitutional fail-closed rule

If an action cannot be classified, policy state is unavailable, a transition is
ambiguous, a state rule is duplicated, or evidence/approval verification fails,
the action is treated as constitutional or denied—never as routine.

### S6 — continuity non-enrichment

Continuity authority cannot:

- issue, cancel, dilute, or transfer ownership;
- materially increase compensation of the invoking participant;
- transfer IP or assets to a related party;
- weaken governance policy;
- delete audit history;
- change recovery beneficiaries;
- sell the company or substantially all assets;
- create unusual debt outside a separately approved policy.

### S7 — equivalent-access recovery

Credential recovery may replace a lost credential with equivalent or narrower
roles and scope. It cannot recover, export, or reconstruct the lost private key,
or upgrade the recovered participant's authority.

### S8 — state-transition authorization

A continuity state changes only through an allowed edge whose evidence,
attestations, notices, challenge window, quorum, and current-state precondition
all hold. A stale transition request cannot apply after another transition wins.

### S9 — bounded delegated authority

Emergency or continuity authority is scoped to explicit action kinds, amounts,
resources, tenant, proposal, state generation, and expiry. It automatically
expires and cannot be renewed by the delegated actor alone.

### S10 — fenced single execution

An external mutation executes only under a current lease/fencing token. After a
newer token is issued, a holder of an older token cannot begin or finalize the
protected action. The provider adapter must carry an idempotency key or reconcile
ambiguous results before retrying.

### S11 — durable replay protection

A proposal nonce, proposal ID, approval, execution idempotency key, and receipt
cannot be reused for another action. Replay state survives failover, snapshot,
and restore.

### S12 — append-only evidence

Material policies, proposals, approvals, transitions, execution attempts,
provider results, challenges, and restorations are linked by hashes or an
equivalent tamper-evident sequence. Corrections append new records; they do not
rewrite history.

### S13 — no silent off-platform authority

Material provider drift creates an audit event and alert. When technically
possible, Fiducia revokes or freezes affected capabilities until the drift is
reconciled under policy.

### S14 — tenant isolation

No participant, approval, credential, proposal, policy, transition, executor
lease, idempotency record, or receipt from tenant X can authorize an action for
tenant Y.

## Liveness invariants

### L1 — bounded routine continuity

When one founder is unavailable, one authorized active founder can complete a
pre-approved, bounded routine action without the unavailable founder, provided
all continuity-state requirements hold.

### L2 — defensive emergency action

A compromised credential or active incident can be contained promptly through a
narrow emergency path, while irreversible economic and constitutional actions
remain blocked.

### L3 — recoverability without key reconstruction

Loss of all devices for one participant has a finite recovery path using the
configured independent quorum, notice, cooldown, and equivalent-access rule.

### L4 — restoration

A temporarily unavailable or provisionally incapacitated founder has a finite,
protected path to challenge the state, register a replacement credential, and
restore the prior role set when the agreed restoration conditions hold.

### L5 — succession

Death or confirmed long-term incapacity has a finite path to an estate
representative, successor, passive ownership, or agreed buyout process without
requiring the unavailable participant's private key.

### L6 — nonparticipation resolution

Repeated authenticated notice, cure period, and neutral review can suspend a
routine operational veto for bounded actions. Silence alone never authorizes a
constitutional action.

### L7 — deadlock termination

A genuine 50:50 disagreement has a finite escalation path: structured proposals,
mediation, domain expert, independent tie-break process, contractual buy-sell,
orderly sale, or dissolution. The neutral role receives only the power defined
for that stage.

### L8 — partition recovery

After a network partition heals, exactly one effective continuity state and one
executor fencing generation prevail. No approval or execution accepted only on a
minority partition becomes valid merely because the partition healed.

## Action/state authorization matrix

This is a conservative default, not a universal legal template.

| State | Routine | Sensitive | Constitutional | Recovery | Emergency |
| --- | --- | --- | --- | --- | --- |
| Normal | One authorized operator within limits | Both founders or configured strong quorum | All protected stakeholders under governing documents | Other founder + guardian, notice and cooldown | Narrow configured emergency quorum |
| Temporarily unavailable | Available authorized founder within strict limits | Founder + guardian, delayed and scoped | Denied | Founder + guardian, equivalent access only | Founder + guardian, immediate defensive scope only |
| Provisional incapacity | Available founder within strict limits | Founder + guardian, delayed and scoped | Denied | Founder + guardian + required attestation | Defensive scope only |
| Confirmed long-term incapacity | Available founder for maintenance | Only through successor/estate procedure | Denied until legally authorized successor representation | Activate successor/estate identity | Defensive scope only |
| Voluntary exit | Remaining operators after transition effective time | Per signed transition | Per governing documents | Credential rotation and role removal | Defensive scope only |
| Nonparticipation | Bounded continuity after notice/cure/neutral review | Generally denied or independently reviewed | Denied | Recovery only if credential loss is separately established | Defensive scope only |
| Active deadlock | Existing approved operations only | Frozen unless deadlock procedure expressly covers action | Denied until tie-break/buy-sell/legal procedure | Defensive recovery only | Defensive scope only |
| Succession | Authorized successor/estate operating rule | Successor + required protected stakeholders | Governing-document quorum | Successor credential lifecycle | Defensive scope only |
| Restored | Return to normal effective policy | Return to normal effective policy | Return to normal effective policy | Standard recovery | Standard emergency rule |

## Required negative tests

1. Founder A submits two credentials and attempts to satisfy a two-person quorum.
2. Founder B changes `participant_role` to `guardian` in an approval.
3. A revoked guardian approval arrives after revocation but before execution.
4. A proposal parameter changes after the first approval.
5. A policy replacement removes a notice recipient or shortens a challenge window.
6. A policy contains two rules for the same continuity state.
7. A stale executor calls the provider after a newer fencing token is issued.
8. The provider times out after applying the action; retry must reconcile instead
   of applying twice.
9. A minority partition approves a transition and later rejoins.
10. A founder falsely claims death or incapacity and attempts a self-dealing
    transaction during the challenge window.
11. Provider support changes the root owner outside Fiducia.
12. A tenant-X credential or approval is replayed against tenant Y.
13. Snapshot/restore regresses nonce, fencing, or receipt high-water state.
14. The active founder attempts to renew their own temporary authority indefinitely.
15. An unclassified provider action is submitted as routine.

## Required positive tests

1. Both founders approve the exact normal-state sensitive proposal and it executes once.
2. An available founder completes a bounded routine action during verified temporary
   unavailability.
3. Founder + guardian replace a lost founder credential after notice and cooldown,
   with equivalent roles only.
4. A recovered founder successfully challenges temporary authority and returns to
   normal state.
5. A successor or estate identity is activated without reconstructing the deceased
   founder's key.
6. Emergency authority revokes a compromised credential immediately but cannot
   issue equity or transfer IP.
7. A partition heals and the highest committed state/fencing generation wins.
8. An ambiguous provider result is reconciled into one final receipt.

## Provider bypass analysis

Every connector must answer:

- Who can change the root owner outside the API?
- Can support reset MFA, email, domain, billing, or organization ownership?
- Can a founder retain a personal recovery channel?
- Are service credentials non-exportable or merely stored secrets?
- Which actions expose idempotency or conditional-update primitives?
- Which material states can be polled for drift?
- Can provider audit logs be exported to an independent sink?
- What is the containment action when drift is detected?

A connector must not advertise hard enforcement where the provider exposes an
undetectable or unavoidable unilateral recovery path.

## Legal and organizational boundary

The technical system should record references to the governing documents and the
role/version they implement. Qualified counsel must determine:

- whether software approvals constitute valid corporate or LLC consent;
- who may attest death, incapacity, abandonment, or succession;
- how voting rights and economic ownership are treated on death or departure;
- whether a guardian is a director, manager, trustee, voting trustee, escrow
  agent, attorney-in-fact, or other fiduciary;
- the enforceability of buy-sell, mediation, arbitration, and deadlock clauses;
- required notices, records, signatures, retention, and jurisdiction.

Fiducia must never label a technical state as a final legal determination unless
the governing process expressly gives it that effect.

## Residual risks and open decisions

- Guardian independence and liability model.
- Whether constitutional actions require the guardian in addition to founders.
- Threshold cryptography versus HSM-mediated execution credentials.
- Multi-region time source and timelock semantics during partition.
- How to prove notification delivery rather than mere send attempts.
- How to handle a guardian who dies, disappears, or is compromised.
- Whether recovery can change credential type or only credential instance.
- Provider-specific limits on root-account custody and support bypass.
- Privacy and retention requirements for medical, estate, or dispute evidence.
- Jurisdiction-specific interpretation of electronic signatures and voting.

## Discovery exit criteria

DEN-175 is ready to close only when:

- each S/L invariant is represented by an executable property, model, or test;
- normal, temporary-unavailability, incapacity, succession, restoration, and
  deadlock paths are modeled;
- a one-founder constitutional takeover has no reachable success state;
- a bounded routine action remains reachable with one founder unavailable;
- stale executors and minority partitions cannot create valid effects;
- unresolved trust assumptions and provider/legal gaps remain explicit.
