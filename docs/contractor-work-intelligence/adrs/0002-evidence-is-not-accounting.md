# ADR-0002: Evidence and model inference are not accounting facts

- **Status:** Accepted
- **Date:** 2026-07-31
- **Owners:** DEN-992, DEN-995, DEN-996, DEN-997, DEN-999

## Context

The product combines explicit user controls with imperfect signals such as verbal
cues, non-speech sound classifications, location, motion, schedules, photos,
receipts, and imported work orders. These inputs can help reconstruct a workday,
but they cannot reliably determine billability, payable time, contract scope,
customer consent, or a completed task without human context.

A drill sound may indicate active work, testing, a neighboring contractor, or a
personal task. Silence may indicate planning, diagnosis, cleanup, travel, or a
sensor failure. A geofence exit may indicate a billable material run, lunch, or a
misconfigured site boundary.

Allowing probabilistic signals to create business consequences directly would
produce inaccurate invoices, wage decisions, reports, and disputes while
encouraging surveillance-oriented product behavior.

## Decision

Use a strict, explicit trust pipeline:

```text
Evidence item / observation
    -> candidate activity or extracted value
    -> append-only human review event
    -> approved accounting/business fact
    -> report, timesheet, or invoice draft
```

- Observations record source events and provenance.
- Rules and models produce candidates with confidence, alternatives, versions,
  configuration, and source IDs.
- Authorized humans approve, reject, or correct candidates.
- Approved projections preserve raw candidate values separately from approved,
  payable, and billable values.
- Customer-facing statements require human-approved grounding.
- Invoice lines reference approved accounting records and deterministic policies.

No API, database foreign key, UI bulk action, background worker, or integration may
skip these stages.

## Required invariants

1. Candidate IDs cannot satisfy approved-time or invoice references.
2. A rejected review cannot produce an approved record.
3. Unknown/non-billable review state projects zero billable quantity.
4. Raw candidate duration remains visible after correction.
5. Reports cannot use a model candidate as the sole source of an external claim.
6. Observations may provide provenance, but external factual statements also need
   a non-rejected human review or approved record.
7. Monetary values are calculated by deterministic, versioned code.
8. Payroll, discipline, and invoice issuance require separate consequence-domain
   approvals beyond sensor review.
9. Corrections append and supersede; they do not rewrite source history.
10. Model rollback can invalidate/regenerate candidates without changing existing
    human approvals silently.

## Consequences

### Positive

- errors remain reviewable rather than consequential;
- workers can understand and correct the system;
- reports and invoices have defensible provenance;
- model quality can improve from correction labels;
- deterministic billing is testable across runtimes;
- privacy policy can prefer derived events without pretending they are facts.

### Negative

- every workflow includes a review step;
- review burden may reduce adoption if candidate quality or UX is poor;
- “fully automatic invoicing” is not an acceptable initial marketing promise;
- additional event, projection, and audit storage is required;
- conflicts need explicit resolution.

## Alternatives considered

### Auto-approve high-confidence candidates

Rejected as a general rule. Confidence calibration differs across devices,
trades, environments, and categories. A future organization policy may allow
risk-bounded bulk review, but the action remains an authorized approval with an
audit event and preview.

### Treat explicit verbal cues as unquestionable facts

Rejected. Cues can be misrecognized, triggered by media/other speakers, or refer
to a different job. Explicit controls are stronger evidence and can receive a
lighter confirmation flow, but consequence-bearing values remain reviewable.

### Let an LLM calculate and issue the invoice

Rejected. Language models are unsuitable as the authority for deterministic
quantities, rates, taxes, and totals.

### Keep only the final approved result

Rejected. That destroys provenance, makes model evaluation impossible, and hides
manager/worker corrections.

## Implementation guidance

- Use separate record types and tables/collections for observations, candidates,
  reviews, and approved projections.
- Use schema and semantic validation, not naming conventions alone.
- Require source IDs in report statements and invoice lines.
- Preserve actor, policy, producer, model, and calculation versions.
- Display candidate and approved records differently in the UI.
- Make every bulk approval generate explicit review events.
- Test negative paths at contract, domain, API, and E2E levels.

## Review trigger

This ADR may be extended to support explicitly authorized deterministic sources
that already constitute business facts, such as a signed fixed-price milestone or
an imported approved accounting record. It must not be weakened merely because a
model appears accurate in a limited dataset.