# Founder Partnership Control Plane

**Status:** Discovery proposal  
**Linear:** DEN-158  
**Scope:** Fiducia cross-cutting architecture

## Product invariant

> No founder can unilaterally dispossess another founder, and no founder can permanently hold the company hostage outside a transparent process agreed while trust was high.

This capability guarantees process and technical enforcement over connected systems. It does not replace corporate, securities, estate, employment, fiduciary, or contract law.

## Rights must remain separate

The system must represent these as independent rights:

1. **Economic ownership** — shares, distributions, and sale proceeds
2. **Constitutional voting authority** — equity issuance, merger, IP sale, governance amendments
3. **Governance role** — director, manager, officer, or authorized fiduciary
4. **Operational access** — source control, cloud, DNS, identity, banking, payroll, app stores, and production

Operational unavailability must not erase vested ownership. Economic ownership must not imply an indefinite veto over routine operations.

## Safety and liveness

A fixed 2-of-2 rule maximizes unilateral-takeover resistance but fails liveness. A fixed 1-of-2 rule preserves liveness but enables unilateral control. Fiducia should implement action-specific and state-specific quorum policies.

### Safety properties

- A single founder cannot weaken a protected quorum.
- Recovery restores equivalent access; it cannot silently create superior access.
- Continuity powers cannot transfer economic value to the person invoking continuity.
- An approver signs the exact immutable proposal, parameters, policy version, nonce, and expiry.
- A modified proposal invalidates all prior approvals.
- Revoked, stale, or superseded executors cannot complete protected actions.
- Audit records are append-only and externally verifiable.

### Liveness properties

- Routine bounded operations can continue during temporary unavailability.
- Lost credentials can be replaced without reconstructing another person's private key.
- Death and long-term incapacity activate a successor or estate-representation path.
- Voluntary departure can remove operational duties without automatically forfeiting vested ownership.
- Nonparticipation and deadlock have a finite escalation path.
- Temporary continuity authority expires automatically unless renewed through the required process.

## Action classes

| Class | Examples | Default policy |
| --- | --- | --- |
| Routine | Approved deployment, ordinary vendor payment, standard operational task | One authorized operator within policy limits |
| Sensitive | Secret rotation, cloud-admin change, bank beneficiary, production policy change | Both founders or configured strong quorum |
| Constitutional | Equity issuance, dilution, ownership transfer, core-IP sale, quorum weakening | All protected stakeholders; no ordinary emergency bypass |
| Recovery | Replace lost key, activate equivalent successor identity | Remaining founder plus independent guardian, notice, and cooldown |
| Emergency | Incident containment, credential revocation, payroll and infrastructure continuity | Narrow break-glass capability with scope, expiry, and audit |

A policy may only be weakened by the same or a stronger quorum than the policy currently requires.

## Continuity states

```text
NORMAL
  -> TEMPORARILY_UNAVAILABLE
  -> PROVISIONAL_INCAPACITY
  -> CONFIRMED_LONG_TERM_INCAPACITY
  -> RESTORED | SUCCESSION

NORMAL
  -> VOLUNTARY_EXIT
  -> TRANSITION
  -> PASSIVE_OWNER | BUYOUT | SUCCESSOR

NORMAL
  -> NONPARTICIPATION
  -> NOTICE
  -> CURE_PERIOD
  -> MEDIATION_OR_NEUTRAL_REVIEW
  -> LIMITED_CONTINUITY | BUYOUT | DISSOLUTION

NORMAL
  -> ACTIVE_DEADLOCK
  -> STRUCTURED_PROPOSALS
  -> MEDIATION
  -> EXPERT_OR_INDEPENDENT_TIE_BREAK
  -> BUY_SELL | ORDERLY_SALE | DISSOLUTION
```

Every transition must define the initiator, evidence, attestors, notices, challenge period, permitted powers, prohibited powers, duration, expiration, restoration, appeal, and audit artifacts.

## Independent continuity guardian

A guardian can be an independent director, professional fiduciary, trustee, escrow provider, attorney, or specialized continuity service. Guardian powers must be narrowly scoped.

Permitted examples:

- Confirm that notice and cure procedures completed
- Replace a lost credential with an equivalent credential
- Activate a predesignated successor or estate identity
- Approve bounded operational continuity
- Execute a previously agreed buyout or succession workflow

Forbidden examples:

- Confiscating or canceling vested ownership
- Opportunistic dilution
- Related-party asset transfers
- Materially increasing the active founder's compensation
- Weakening quorum policy
- Deleting audit history
- Selling the company through an ordinary recovery path

## Technical model

### Identities

- Independent WebAuthn/passkey or hardware-key identity per participant
- Multiple registered authenticators per participant
- No shared password, shared private key, or irreplaceable shared device
- Credentials identify a role and legal person; they are not themselves proof of current legal authority

### Proposal

A proposal should include:

```yaml
tenant_id: company-123
proposal_id: uuid
kind: github.change_org_owner
parameters_hash: sha256:...
policy_version: 17
created_by: founder-a
created_at: 2026-07-27T00:00:00Z
expires_at: 2026-07-30T00:00:00Z
nonce: random-256-bit
continuity_state: normal
```

Each approval signs a canonical hash of the complete proposal.

### Policy

```yaml
kind: equity.issue
normal:
  quorum: all_protected_founders
continuity:
  permitted: false
policy_change:
  quorum: current_rule_or_stronger
```

```yaml
kind: credential.replace
normal:
  quorum:
    - one_other_founder
    - one_guardian
  cooldown: 72h
  notice: all_protected_parties
constraints:
  equivalent_access_only: true
  cannot_modify_ownership: true
```

### Execution

Fiducia or an HSM-backed execution service must control the root integration credential. Approval without custody is advisory and can be bypassed by a founder who retains a raw root credential.

Use:

- Strongly consistent proposal and approval state
- Leases and fencing tokens for one active executor
- Idempotency keys for external calls
- Policy-version pinning
- Timelocks and challenge windows
- Signed execution receipts
- Drift detection for off-platform privilege changes

## Initial integrations

1. GitHub organization ownership, repository administration, protected branches, and secrets
2. Cloudflare account, DNS, registrar, and access policy
3. One cloud provider's IAM and root-sensitive operations
4. Identity provider administration and recovery settings

Financial and cap-table integrations should follow only after the legal and provider-specific enforcement model is understood.

## Threat model

At minimum test:

- Malicious Founder A acting alone
- Malicious Founder B acting alone
- Founder plus guardian collusion
- Compromised founder credential
- Compromised guardian credential
- Lost all credentials for one participant
- External provider support bypassing Fiducia
- Stale executor after failover or network partition
- Proposal mutation after partial approval
- Replay of an old approval
- Clock skew during a timelock
- False death, incapacity, or abandonment claim
- Returning founder challenging temporary authority
- Insider attempting self-dealing during continuity mode

## MVP boundary

The MVP should enforce operational control, not attempt to rewrite a legal cap table.

1. Enroll two founders and an optional guardian with independent credentials.
2. Classify protected actions.
3. Hold one execution credential for GitHub and one cloud/DNS integration.
4. Require policy-defined approval.
5. Produce signed receipts and immutable audit history.
6. Implement equivalent-access lost-key recovery with notice and cooldown.
7. Simulate unavailability, compromise, collusion, and network partitions.

## Discovery exit criteria

- Threat model and trust boundaries reviewed
- Safety and liveness invariants represented as testable properties
- Versioned policy schema drafted
- Canonical proposal and signature format drafted
- Continuity state machine formally specified
- GitHub connector proof of concept completed
- One cloud/DNS connector proof of concept completed
- One-founder takeover attempt rejected
- Bounded continuity operation succeeds while the other founder is unavailable
- Recovery never reconstructs another person's key
- Constitutional actions remain blocked in routine continuity mode
- Legal integration points identified and reviewed by qualified counsel
