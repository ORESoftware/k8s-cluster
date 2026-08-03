# Contractor Work Intelligence sister app

> **Working category, not a final product name.** Branding, launch trade, buyer,
> pricing, and the eventual GitHub organization remain owned by Linear DEN-990.

This handbook documents the planned Sonus Auris sister product for contractors,
owner-operators, and small field-service crews. The product turns a visibly
started, job-scoped stream of timestamps, explicit actions, notes, photos,
location/motion context, verbal cues, and opt-in sound classifications into a
reviewable work timeline, approved time records, grounded reports, and invoice
drafts.

The central rule is intentionally repeated throughout the handbook:

> **Sensor observations are evidence. Machine-generated activity spans are
> proposals. Human-approved time entries are accounting facts.**

No sound, transcript, geofence transition, calendar entry, or model output may
directly create payroll, a customer invoice, a disciplinary action, or a
customer-facing factual claim.

## Documentation status

| Area | Status | Canonical document |
| --- | --- | --- |
| Product promise, users, scope, metrics | Incubation baseline | [Product brief](PRODUCT.md) |
| System context and component boundaries | Incubation baseline | [Architecture](ARCHITECTURE.md) |
| Offline envelopes, replay, causality, attachments | Implemented contract + service roadmap | [Offline sync protocol](OFFLINE_SYNC_PROTOCOL.md) |
| Entities, states, invariants, lineage | Implemented contract + roadmap | [Domain model](DOMAIN_MODEL.md) |
| Worker, reviewer, and customer workflows | Design baseline | [User experience](USER_EXPERIENCE.md) |
| Consent, retention, redaction, anti-surveillance | Mandatory product policy | [Privacy and trust](PRIVACY_AND_TRUST.md) |
| Reports, rate cards, deterministic invoices | Incubation baseline | [Reports and billing](REPORTS_AND_BILLING.md) |
| Tests, observability, support, rollout gates | Engineering baseline | [Operations and quality](OPERATIONS_AND_QUALITY.md) |
| Delivery phases and extraction plan | Active roadmap | [Roadmap](ROADMAP.md) |
| Shared vocabulary | Maintained reference | [Glossary](GLOSSARY.md) |

Architectural decisions are recorded under [`adrs/`](adrs/):

1. [ADR-0001: keep the contractor product separate from Sonus Auris](adrs/0001-separate-sister-product.md)
2. [ADR-0002: evidence is not an accounting fact](adrs/0002-evidence-is-not-accounting.md)
3. [ADR-0003: local-first capture with selective encrypted synchronization](adrs/0003-local-first-selective-sync.md)

## Canonical implementation references

The handbook describes the intended product as a whole. The following artifacts
are normative for the parts they already implement:

- [`apps/sonus-auris-interfaces/incubation/contractor-work-intelligence/v1/`](../../apps/sonus-auris-interfaces/incubation/contractor-work-intelligence/v1/)
  contains the versioned v1 work-ledger and offline-sync schemas, semantic
  validators, golden workday/offline batches, and negative billing, privacy,
  replay, lineage, and attachment tests.
- Linear [DEN-989](https://linear.app/denman/issue/DEN-989) is the parent product
  program.
- Linear [DEN-990](https://linear.app/denman/issue/DEN-990) owns market, launch
  vertical, pricing, and naming discovery.
- Linear [DEN-991](https://linear.app/denman/issue/DEN-991) owns repository and
  shared-component architecture.
- Linear [DEN-992](https://linear.app/denman/issue/DEN-992) owns portable domain
  contracts.
- Linear [DEN-1542](https://linear.app/denman/issue/DEN-1542) owns the offline
  envelope, replay, causal-ordering, and encrypted-attachment contract slice.
- Linear [DEN-994](https://linear.app/denman/issue/DEN-994) owns sonic and verbal
  cue recognition.
- Linear [DEN-996](https://linear.app/denman/issue/DEN-996) owns generated field
  reports.
- Linear [DEN-997](https://linear.app/denman/issue/DEN-997) owns deterministic
  billing.
- Linear [DEN-999](https://linear.app/denman/issue/DEN-999) owns privacy, consent,
  retention, redaction, and labor-trust controls.
- Linear [DEN-1001](https://linear.app/denman/issue/DEN-1001) owns extraction of
  reusable Sonus capture and provenance capabilities.
- Linear [DEN-1002](https://linear.app/denman/issue/DEN-1002) owns exports and
  business-system integrations.

When prose and a versioned contract disagree, the contract governs software that
claims compatibility with that version. The discrepancy must also produce a
documentation or contract issue; silent divergence is not acceptable.

## Reading paths

### Product and pilot teams

Read [Product brief](PRODUCT.md), [User experience](USER_EXPERIENCE.md),
[Privacy and trust](PRIVACY_AND_TRUST.md), and [Roadmap](ROADMAP.md).

### Application and backend engineers

Read [Architecture](ARCHITECTURE.md), [Domain model](DOMAIN_MODEL.md),
[Offline sync protocol](OFFLINE_SYNC_PROTOCOL.md),
[Reports and billing](REPORTS_AND_BILLING.md), and
[Operations and quality](OPERATIONS_AND_QUALITY.md), then inspect the versioned
ledger and sync contracts.

### Security, privacy, legal, and labor reviewers

Read [Privacy and trust](PRIVACY_AND_TRUST.md), ADR-0002, ADR-0003, the data
classification section in [Architecture](ARCHITECTURE.md), the evidence-lifecycle
and attachment sections in [Offline sync protocol](OFFLINE_SYNC_PROTOCOL.md), and
every rollout gate in [Operations and quality](OPERATIONS_AND_QUALITY.md).

### Model and audio engineers

Read the observation and inference boundaries in [Domain model](DOMAIN_MODEL.md),
the capture lifecycle in [User experience](USER_EXPERIENCE.md), the model
telemetry restrictions in [Privacy and trust](PRIVACY_AND_TRUST.md), and the
evaluation matrix in [Operations and quality](OPERATIONS_AND_QUALITY.md).

## Documentation rules

- Keep the product name provisional until DEN-990 is resolved.
- Do not document automatic billing, payroll, discipline, or worker scoring from
  ambient signals; those behaviors are prohibited.
- Mark implemented behavior, accepted design, hypothesis, and future work
  distinctly.
- Every externally visible generated fact must have a documented source and
  approval path.
- Every monetary example must use deterministic arithmetic and state its rounding
  policy.
- Every feature that captures sound or location must document visible state,
  pause/stop behavior, retention, sharing, and deletion.
- Every sync feature must document local durability, stable identity, replay,
  conflict, partial failure, attachment encryption, and user-visible status.
- Update the handbook in the same PR as a material contract or architecture
  change. A code-only semantic change is incomplete.

## Current product maturity

The product is in **incubation**. Portable work-ledger and offline-sync contracts
and semantic safety rules exist, but the final organization, repositories, cloud
environments, mobile application, production API/sync service, reports, invoices,
and pilot integrations do not yet constitute a released product. This handbook
is a design and engineering contract for building those pieces, not a statement
that they are already live.
