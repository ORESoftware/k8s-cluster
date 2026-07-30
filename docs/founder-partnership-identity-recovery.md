# Founder Partnership Control Plane — identity and recovery architecture

**Status:** Discovery draft  
**Linear:** DEN-158, DEN-177  
**Related:** `docs/founder-partnership-control-plane.md`,
`docs/founder-partnership-threat-model.md`

## Repository audit

The current Fiducia authentication boundary is suitable for ordinary product
sessions but not yet for founder-governance signatures:

- `fiducia-auth.rs` treats Supabase as the source of truth for human identity and
  organization membership.
- Human callers present Supabase session JWTs. Fiducia verifies them through
  cached JWKS or the configured Supabase user endpoint.
- `UserCtx` already carries `aal1` or `aal2`, and downstream customer surfaces can
  require a live MFA factor for step-up.
- `fiducia-auth.rs` owns B2B API keys and short-lived service JWTs, not WebAuthn
  credentials or multi-party governance approvals.
- The current crate manifest has no WebAuthn/FIDO dependency.

Therefore, Supabase login should remain the ordinary session and organization
membership layer. A separate Fiducia governance credential layer must authorize
protected proposals. A valid Supabase session—even an `aal2` session—may open the
approval user interface, but it must not count as the approval itself.

## Four identity layers

### 1. Session identity

Purpose: establish who is using the Fiducia dashboard and which tenant they may
access.

Current authority:

- Supabase user ID and verified JWT
- trusted organization membership
- trusted application roles from server-controlled metadata
- session assurance level (`aal1` or `aal2`)

Session identity is revocable and short-lived. It is not a durable governance
signature.

### 2. Governance participant identity

Purpose: represent a protected person or continuity role in one tenant's
constitution.

Examples:

- Founder A
- Founder B
- guardian / continuity fiduciary
- estate representative
- designated successor
- bounded operator

The tenant-controlled participant registry binds:

- participant ID
- legal/external identity reference
- allowed governance roles
- registered authenticator IDs
- active, suspended, revoked, or succeeded status
- validity interval
- predecessor/successor relationships
- governing-policy version that authorized the record

A role written inside an approval payload is an untrusted assertion until it
matches this registry.

### 3. Authenticator identity

Purpose: prove that a participant approved an exact proposal.

Each participant uses independent WebAuthn credentials or another approved
public-key authenticator. Fiducia stores only public credential material and
metadata. It never stores, exports, reconstructs, or shares the participant's
private key.

### 4. External legal/fiduciary authority

Purpose: establish facts that software cannot determine, including death,
incapacity, estate authority, corporate office, and legally effective
succession.

Fiducia stores immutable references and attestations. It does not claim that a
cryptographic credential alone creates or transfers legal ownership or office.

## Service boundary

### Recommended initial implementation

Add a logically isolated governance-auth module to `fiducia-auth.rs` for the
MVP, with:

- separate routes;
- separate durable namespace;
- separate encryption/KMS keys;
- separate authorization middleware;
- separate audit stream;
- no reuse of customer API-key secrets or signing roots.

This reuses established Supabase session verification and tenant resolution
without pretending the Supabase session is the governance credential.

Extract the module into a dedicated governance-auth service only when one of
these conditions appears:

- independent scaling or availability requirements;
- separate compliance boundary;
- HSM ownership cannot be shared with ordinary auth;
- release cadence or operator access must be isolated;
- provider/tenant policy requires a dedicated single-tenant deployment.

### Why not store governance roles only in Supabase metadata

Server-controlled metadata is useful for application access but is too broad as
the sole constitutional source of truth:

- it does not bind an approval to an exact proposal hash;
- provider support or service-role access may change it outside the Fiducia
  quorum process;
- it lacks the required policy version, notices, challenge windows, succession,
  and tamper-evident approval history;
- one user may hold different governance roles in different tenants;
- a participant's legal authority may outlive or differ from product login
  access.

Supabase identity should link to, not replace, the Fiducia participant record.

## Credential record

A governance credential record should include at least:

```yaml
tenant_id: company-123
participant_id: founder-a
credential_id: base64url-webauthn-id
public_key_cose: encrypted-or-binary-public-material
signature_counter: 17
credential_class: device_bound | backup_eligible | synced
transports: [usb, nfc]
aaguid: optional-authenticator-guid
user_verification_required: true
backup_eligible: false
backup_state: false
status: active | suspended | revoked | replaced
created_at_ms: 1785177600000
last_used_at_ms: 1785264000000
replaced_by_credential_id: null
authorized_by_policy_hash: sha256:...
```

The record must not treat authenticator attestation as proof of a person's legal
identity. It proves possession/control of an authenticator registered through a
particular ceremony.

## Authenticator policy tiers

A tenant should configure credential requirements by action class.

### Routine

- Valid product session
- Optional governance signature depending on tenant policy
- Synced or device-bound credentials may be acceptable

### Sensitive

- Fresh product step-up session
- WebAuthn user verification required
- Registered active participant credential
- Proposal-bound challenge
- Tenant may disallow credentials currently marked as backed up or synced

### Constitutional

Conservative default:

- WebAuthn user verification required
- at least one independently registered credential per required participant
- hardware/device-bound credential required when reliably enforceable
- no recovery credential may approve the same constitutional transaction that
  created or elevated it
- fresh challenge, explicit human-readable transaction summary, cooldown, and
  out-of-band notice

Credential metadata and authenticator flags can inform policy, but the product
must document what can and cannot be reliably inferred about hardware binding,
synchronization, cloning, or provider account recovery.

## Challenge and approval protocol

### Begin approval

1. Caller presents a valid tenant-scoped product session.
2. Fiducia loads the immutable proposal and effective policy from strongly
   consistent state.
3. Fiducia confirms the participant is eligible to approve that action.
4. Fiducia creates a one-time challenge committing to:
   - tenant ID;
   - proposal ID;
   - canonical proposal hash;
   - policy hash and version;
   - continuity state and generation;
   - participant ID;
   - credential allowlist;
   - RP ID and expected origin;
   - nonce, issue time, and expiry.
5. The challenge is durably claimed so two replicas cannot consume it twice.

### Finish approval

1. Verify challenge existence, tenant, participant, purpose, and expiry.
2. Verify RP ID, expected origin, client-data challenge, authenticator data,
   signature, and user-verification requirement.
3. Verify the credential is active and registered to the participant.
4. Evaluate signature counter and backup-state changes according to policy;
   suspicious changes trigger review rather than silently granting authority.
5. Atomically mark the challenge consumed and append the approval.
6. Emit a signed receipt referencing the exact proposal hash and credential.

A product session authorizes access to this protocol. Only the verified
WebAuthn assertion creates the governance approval.

## Enrollment ceremonies

### Initial company bootstrap

The first constitution cannot be authorized by a policy that does not yet exist.
Use an explicit bootstrap ceremony:

1. Verify tenant formation and participant identities through the configured
   legal/onboarding process.
2. Enroll at least two credentials for each protected founder when feasible:
   primary and separately stored backup.
3. Enroll the guardian independently, through a different administrator and
   communication channel.
4. Display and sign the initial policy hash, participant registry hash, recovery
   policy, and provider-custody inventory.
5. Export an externally verifiable bootstrap receipt to every protected party.
6. Permanently disable the bootstrap authority after the first constitution is
   committed.

Bootstrap credentials must never remain as a hidden super-admin bypass.

### Add a routine backup credential

- Require a fresh assertion from an existing active credential of the same
  participant.
- Notify every protected party.
- Apply a cooldown before the new credential may approve sensitive or
  constitutional actions.
- Prevent the new credential from approving its own enrollment record.

### Add or replace a founder's final credential

When no active credential remains, use the recovery protocol below. Do not
silently downgrade to email, SMS, password, or a product-session-only flow.

## Equivalent-access recovery

Recovery must restore liveness without creating superior authority.

### Recovery request

A recovery request commits to:

- tenant and participant;
- lost/compromised credential IDs, when known;
- requested replacement credential and properties;
- existing roles and scope being restored;
- effective policy and continuity generation;
- evidence and attestation references;
- notice recipients;
- challenge/cooldown duration;
- expiry and nonce.

### Default quorum

For a two-founder company:

- one other active founder; and
- the independent guardian.

The recovering participant may participate in proof-of-identity steps but does
not alone authorize their own replacement.

### Recovery constraints

- Replacement roles must be a subset of the participant's previously authorized
  roles.
- The recovery path cannot add ownership, guardian, estate, or policy-admin roles.
- It cannot change the participant ID, economic interest, or constitutional vote.
- It cannot weaken policy, remove notices, or shorten the cooldown.
- It cannot reconstruct or export the old private key.
- The replacement credential starts with a sensitive-action quarantine/cooldown.
- The old credential becomes revoked or suspended before the replacement becomes
  fully active, according to the compromise scenario.
- The recovery authority expires automatically and cannot be renewed by the
  recovering participant or active founder alone.

### Lost versus compromised credential

**Lost but not suspected compromised**

- Keep the old credential suspended during the challenge period.
- Notify all protected participants.
- Activate the replacement after quorum and cooldown.
- Revoke the old credential when recovery finalizes.

**Known or suspected compromise**

- Permit immediate defensive suspension through the emergency policy.
- Do not immediately grant a replacement with constitutional authority.
- Require full recovery quorum, notices, and cooldown for equivalent access.
- Review actions performed since the last trusted use.

## Guardian lifecycle

### Enrollment

- Guardian identity is separate from founders and operators.
- Enrollment requires all protected founders under normal policy.
- Guardian credentials use separate devices and recovery channels.
- The guardian receives no routine operational or economic authority.

### Rotation

- Normal rotation requires current guardian plus all protected founders, or the
  predeclared guardian-replacement policy.
- New guardian credentials cannot approve the transaction that installed them.
- Old and new guardian overlap is bounded and visible.

### Guardian unavailable

A guardian-unavailable procedure must be configured before the event. Possible
patterns include:

- two independent guardians with a role quorum;
- primary and successor professional fiduciaries;
- court/contract-defined replacement process;
- time-delayed all-founder replacement while all founders are active.

Do not allow one founder to appoint a replacement guardian during conflict.

### Guardian compromise or collusion

Even a valid guardian credential cannot:

- issue or cancel equity;
- transfer founder roles;
- weaken policy;
- remove audit history;
- approve related-party transfers;
- approve a constitutional action with only one founder;
- convert temporary authority into permanent authority.

The participant registry and action policy must enforce these limitations rather
than relying on the guardian's contractual promise.

## Death, incapacity, and succession

### Private keys are not inherited

Never attempt to recover a deceased or incapacitated participant's private key.
Revoke the credential and activate a new identity for the estate representative
or successor after the required external and internal process.

### Estate/successor identity

The new participant record should link to:

- predecessor participant ID;
- external authority references;
- effective date and expiry/review date;
- allowed roles;
- exact policy/transition that activated it.

Economic ownership records remain in the legally authoritative system. Fiducia
records and enforces the technical role granted by the governing process.

### Restoration after temporary incapacity

- The returning founder initiates a restoration challenge with an active or
  newly recovered credential.
- The guardian and required protected parties receive notice.
- Temporary delegated authority is revoked at the restoration generation.
- Any in-flight executor holding the old generation is fenced out.
- The returning founder receives the same or narrower prior role set unless a
  separate constitutional process changed it.

## Credential and session separation

| Event | Product session | Governance credential |
| --- | --- | --- |
| Password reset | May restore dashboard login | Must not restore governance approval authority |
| Supabase MFA reset | Changes product AAL path | Must not replace founder credential |
| Email change | Changes login/contact after verification | Must not change participant identity or role |
| Session theft | May expose UI and proposals | Cannot approve without WebAuthn assertion |
| WebAuthn key loss | Product login may continue | Requires governance recovery process |
| Founder role revocation | Product membership may continue | Governance registry/policy controls approval role |

## Storage and availability

- Store public credential records, challenge state, approvals, revocations, and
  participant registry in strongly consistent Fiducia state.
- Encrypt credential metadata and sensitive evidence references at rest.
- Keep raw medical, estate, or dispute documents outside the hot authorization
  path; store content hashes and access-controlled references.
- Replicate revocation and challenge-consumption state before acknowledging.
- Bind every authorization to a continuity-state generation.
- Preserve a durable credential/version high-water mark across snapshot restore.

## Audit events

At minimum append events for:

- participant created, suspended, restored, succeeded, or revoked;
- credential enrollment begun/completed/failed;
- credential suspended, revoked, replaced, or backup-state changed;
- approval challenge issued, consumed, expired, or replayed;
- governance approval accepted or rejected with safe reason code;
- recovery request created, challenged, approved, canceled, or finalized;
- guardian enrollment/rotation/replacement;
- external provider or Supabase identity drift;
- state restoration and fencing-generation change.

Audit responses must not expose public keys, raw authenticator data, sensitive
evidence, session tokens, or recovery secrets to unauthorized callers.

## Required tests

1. `aal2` product session without WebAuthn assertion cannot approve.
2. Valid WebAuthn assertion from an unregistered credential cannot approve.
3. Registered founder credential cannot claim guardian role.
4. Duplicate credentials for one participant count as one human approval.
5. Replayed challenge is rejected after failover/restart.
6. Wrong RP ID, origin, tenant, proposal hash, policy hash, or generation is
   rejected.
7. Suspended/revoked/replaced credential is rejected.
8. Credential valid at challenge issuance but revoked before finish is rejected.
9. Recovery replacement cannot gain a role absent from the predecessor record.
10. Founder + guardian recovery cannot perform equity, IP, policy, or audit
    changes.
11. New recovery credential cannot approve its own enrollment.
12. Guardian replacement cannot be authorized by one founder during conflict.
13. Returning founder restoration fences temporary executors.
14. Supabase password/MFA/email recovery does not alter governance credentials.
15. Bootstrap authority cannot be reused after constitution activation.

## Migration plan

### Phase 0 — discovery

- Keep governance schemas outside the canonical generated contract index.
- Validate threats, ceremonies, storage, and provider custody.

### Phase 1 — shadow enrollment

- Add participant and credential records behind a tenant feature flag.
- Permit enrollment and verification in non-enforcing mode.
- Compare Supabase user/tenant identity with governance registry links.

### Phase 2 — approval-required test actions

- Gate sandbox GitHub/cloud actions using governance approvals.
- Keep production provider credentials outside the feature.
- Exercise recovery, revocation, replay, and restoration drills.

### Phase 3 — bounded production enforcement

- Move one low-impact provider credential under Fiducia custody.
- Require governance approval for a narrow sensitive-action set.
- Enable drift detection and emergency freeze.

### Phase 4 — constitutional templates

- Only after specialist legal review and provider bypass analysis, offer
  constitutional policy templates with explicit jurisdiction/provider limits.

## Open decisions

- Whether the MVP module lives inside `fiducia-auth.rs` or a dedicated service.
- Exact WebAuthn library and version after security review.
- Required authenticator classes for each action tier.
- How backup eligibility/state affects policy across platforms.
- Whether attestation is required, optional, or privacy-prohibitive.
- How to deliver and prove independent notices.
- Guardian business model, liability, and jurisdiction.
- Whether a tenant may operate without a guardian and what reduced guarantees
  must be displayed.
- Evidence retention and deletion rules for sensitive incapacity/death records.

## Discovery exit criteria

DEN-177 is ready to close only when:

- enrollment, approval, revocation, recovery, succession, and restoration
  ceremonies are specified;
- Supabase session assurance and governance signatures remain technically
  distinct;
- participant roles and credentials are durably bound and generation-versioned;
- equivalent-access recovery is executable without private-key reconstruction;
- guardian compromise cannot create unilateral constitutional authority;
- the required tests are implemented or linked to implementation tickets;
- HSM/KMS, WebAuthn library, and deployment-boundary decisions are documented.
