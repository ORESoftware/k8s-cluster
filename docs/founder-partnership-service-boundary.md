# Founder Partnership Control Plane — service boundary

**Status:** Architecture decision  
**Linear:** DEN-158, DEN-178

## Decision

Implement founder-governance orchestration as a dedicated application/service in
`fiducia-monorepo`, provisionally named `fiducia-founder-control-plane.rs`.

Do not place provider credentials, WebAuthn approval orchestration, continuity
state, or external SaaS connectors inside `fiducia-brain.rs`.

## Rationale

`fiducia-brain` is the infrastructure control plane for Fiducia's own cluster. It
owns node membership, failure detection, shard placement, leader affinity,
scaling, and rebalancing. Its availability and trust boundary should remain
small, deterministic, and independent of third-party SaaS APIs.

Founder governance is a different control plane:

- tenant-specific participants and constitutions;
- proposal, approval, notice, evidence, and continuity workflows;
- WebAuthn and guardian recovery;
- provider credentials and external API adapters;
- timelocks, challenge windows, audit receipts, and drift reconciliation;
- legal/fiduciary process references.

Combining the two would increase the blast radius of a GitHub, Cloudflare,
identity-provider, or WebAuthn defect and could make Fiducia's cluster-placement
control depend on external provider availability.

## Component responsibilities

### `fiducia-founder-control-plane.rs`

Owns:

- tenant founder-governance policy and continuity state machine;
- immutable proposals and canonical parameter hashes;
- approval and transition workflow orchestration;
- notice/challenge/cooldown scheduling;
- issuance of narrow, expiring, generation-bound execution capabilities;
- provider execution ledger, idempotency, reconciliation, and receipts;
- connector registration and action taxonomy;
- drift evaluation, freezes, and alerts;
- APIs for the customer/admin user interfaces.

Must not:

- verify ordinary Supabase login tokens independently of `fiducia-auth`;
- implement WebAuthn cryptography itself when `fiducia-auth` provides a verified
  governance-approval assertion;
- hold raw provider credentials in ordinary application state;
- determine legal death, incapacity, ownership, or fiduciary authority;
- participate in `fiducia-brain` shard-placement consensus.

### `fiducia-auth.rs`

Owns:

- ordinary Supabase session verification and tenant/user resolution;
- governance participant credential enrollment and WebAuthn ceremony state;
- verification of proposal-bound governance assertions;
- credential status, role/participant binding, revocation, and recovery ceremony;
- safe signed assertion results consumed by the founder control plane.

It must preserve the distinction between an AAL2 product session and a founder's
governance signature.

### `fiducia-node.rs`

Provides strongly consistent primitives for:

- effective policy/version records;
- proposal and approval state;
- continuity state and generation;
- challenge consumption and nonce replay protection;
- executor lease and fencing-token high-water;
- idempotency claims and final receipt references;
- durable timers or timer intents where supported.

The product service should use an explicit namespace and key schema. It should
not embed provider-specific logic into `fiducia-node`.

### `fiducia-brain.rs`

Remains responsible only for Fiducia cluster topology, placement, failover,
scale, and rebalancing. It may expose infrastructure health to the governance
service but does not evaluate founder policy or call provider APIs.

### `fiducia-interfaces`

Owns reviewed canonical contracts once the current non-canonical design schemas
are approved:

- participant and credential metadata;
- policies, proposals, approvals, transitions, evidence and notices;
- delegated authority and execution receipts;
- connector action and drift event envelopes.

Design files remain outside `schema/index.json` until versioning and migration
are accepted.

### Provider-adapter workers

GitHub, Cloudflare, and later adapters should run as isolated workers or modules
with:

- one explicit action allowlist;
- one credential scope;
- no arbitrary URL/method/body execution;
- proposal-to-request canonical regeneration;
- read-before/write/read-after reconciliation;
- current fencing and idempotency checks immediately before mutation;
- no direct customer-facing network exposure;
- separate rate limits, circuit breakers, telemetry, and credential access.

A connector crash or provider outage must not block the core proposal and audit
API.

## Data flow

```text
Dashboard session
      |
      v
fiducia-auth -----------------------------+
  verifies Supabase session               |
  runs WebAuthn approval ceremony          |
  emits proposal-bound verified assertion |
                                             v
                               fiducia-founder-control-plane
                                 policy + continuity workflow
                                 notices + timers + audit
                                 capability issuance
                                             |
                    strongly consistent state / fencing / replay
                                             v
                                       fiducia-node
                                             |
                               scoped execution capability
                                             v
                           isolated provider-adapter worker
                              GitHub / Cloudflare sandbox
                                             |
                               readback + signed receipt
                                             v
                               founder control plane + audit
```

## Provider credential custody

The service stores only credential references and metadata. Provider secrets are
resolved at execution time from KMS/secret-manager custody and made available
only to the specific fenced adapter invocation.

- GitHub App private key: prefer KMS/HSM-backed signing or a secret-manager value
  available only to the GitHub adapter identity. Mint short-lived installation
  tokens after authorization.
- Cloudflare DNS token: encrypted exportable secret, scoped to one sandbox zone,
  available only to the Cloudflare adapter identity.
- Never expose credentials to the dashboard, proposal API, audit records,
  `fiducia-brain`, or general-purpose agent execution.

## Deployment model

For the MVP:

- one founder-control-plane deployment per customer environment or sandbox;
- one strongly consistent governance namespace;
- stateless API replicas;
- a fenced executor worker pool;
- separate connector service identities;
- all provider mutations disabled by default behind tenant feature flags;
- fake transport and sandbox modes required.

Multi-tenant hosting can be evaluated only after tenant isolation, key custody,
rate limiting, and audit export are proven. Customers with sensitive governance
requirements may always require a single-tenant deployment.

## API boundary sketch

```text
POST /v1/governance/proposals
GET  /v1/governance/proposals/{id}
POST /v1/governance/proposals/{id}/approval-challenge
POST /v1/governance/proposals/{id}/approvals
POST /v1/governance/transitions
POST /v1/governance/transitions/{id}/approvals
POST /v1/governance/executions/{proposal_id}
GET  /v1/governance/receipts/{id}
GET  /v1/governance/drift
POST /v1/governance/drift/{id}/freeze
```

The execution endpoint queues or claims an already authorized proposal. It does
not accept arbitrary provider credentials or payloads.

## Failure rules

- Loss of `fiducia-auth`: existing proposals remain readable, but no new human
  approvals are accepted.
- Loss of a provider: workflow state remains available; execution is pending or
  unknown and reconciles later.
- Loss of one API replica: no authority changes because state and challenge
  consumption are strongly consistent.
- Loss of executor lease: older fencing tokens cannot mutate providers.
- Loss of `fiducia-brain`: founder-governance state is not reinterpreted; normal
  infrastructure failover operates independently.
- Provider support bypass: drift monitoring freezes conflicting execution and
  records the discrepancy, but cannot claim cryptographic prevention.

## Initial implementation order

1. Service skeleton with health/readiness and feature flags.
2. Canonical proposal/policy/transition interfaces imported from a reviewed
   `fiducia-interfaces` revision.
3. In-memory/fake state adapter for tests, then `fiducia-node` durable adapter.
4. `fiducia-auth` verified-assertion interface.
5. Timer, notice, challenge, and delegated-authority workflow.
6. Provider-neutral fenced executor and receipt ledger.
7. GitHub ruleset fake and sandbox adapter.
8. Cloudflare DNS fake and sandbox adapter.
9. Drift polling, freeze, and audit export.
10. Adversarial and partition simulations before any production credential.

## Non-goals

- Reusing `fiducia-brain` as a business workflow engine.
- A generic arbitrary-command or arbitrary-HTTP executor.
- Storing provider credentials in proposal or Raft values.
- Enabling owner, Super Administrator, registrar, billing, or cap-table writes in
  the first implementation.
- Claiming the software determines legal authority.
