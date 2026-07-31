# Glossary

This glossary standardizes terms used in product copy, contracts, code, reports,
and support. Avoid using one term for multiple trust levels.

## Accounting fact

A human-approved, policy-valid record that may be used as the basis for a
business consequence such as a timesheet, report, or invoice draft. An
observation or model candidate is not an accounting fact.

## Activity category

A compact classification for a time interval, such as active work, preparation,
travel, material run, break, cleanup, or administration. Categories are not
productivity scores.

## Activity review event

An append-only human decision to approve, reject, or correct a candidate activity
span. A later decision supersedes rather than deletes the earlier event.

## Approved duration

The duration of the final human-approved interval. It may differ from the raw
candidate duration.

## Approved time entry

A versioned projection from one or more candidate spans and non-rejected human
review events. It keeps raw, approved, billable, and payable durations distinct.

## Billable duration

The approved quantity eligible for customer billing under a rate or contract
policy. Billable time is not necessarily payable time.

## Billing engine

Deterministic code that applies versioned rates, quantities, rounding, taxes,
discounts, and other policies. It is not an LLM.

## Candidate activity span

A rule- or model-generated proposal for an interval, category, and possible
billability. It has confidence, alternatives, model/rule version, and source
observations. It requires human review before consequence.

## Capture policy

A versioned policy describing which sensors and evidence types may be used, how
recording is shown, retention, role access, sharing, and worker controls for a job
or organization.

## Content-minimized event

An event containing only the information needed for its purpose, for example a
sound class and confidence without embedding raw audio or a full transcript.

## Correction

A human review action that changes the proposed interval, category, billability,
or related approved data while preserving the original proposal and lineage.

## Customer-sharing grant

A revocable, recipient- and revision-scoped authorization to view a selected
report, invoice, and optional evidence. It is not general tenant access.

## Derived event

An observation produced from local or server processing, such as a recognized
verbal cue or non-speech sound class. It is still evidence, not automatically a
fact about labor or billing.

## Evidence item

Metadata for an optional artifact such as a selected audio clip, photo, receipt,
note, location fact, or imported document. The record contains an opaque locator,
not raw bytes or credentials.

## Frozen revision

An immutable approved report or invoice revision prepared for issue or delivery.
Corrections create a new revision.

## Grounded statement

A report sentence that cites one or more valid source records and includes a
human-approved source for an external factual claim.

## Idempotency key

A stable identity that allows a write or projection to be retried without
creating a duplicate. Reusing a key with different semantics is an error.

## Inference provenance

Metadata describing how a candidate was produced: rule/model/hybrid type, name,
version, configuration hash, confidence, alternatives, and source observations.

## Job

Authorized work for a customer/site under a scope, assignment, schedule, capture
policy, and commercial policy. A job can have multiple job sessions.

## Job session

A worker-controlled, job-scoped period that groups capture state, observations,
candidates, reviews, approved records, evidence, and synchronization.

## Local-first

The device durably records and can review core work without a network connection.
Cloud synchronization adds collaboration and services but is not the sole copy of
new job events.

## Monotonic offset

Elapsed time from a session-local clock that does not move backward with wall-clock
changes. It helps preserve ordering and retention safety.

## Observation

An append-only record that an explicit action, sensor reading, cue, import, or
other input occurred. It may be manual or probabilistic and must preserve source
provenance.

## Opaque locator

A content reference that does not embed credentials, signed query parameters, or
raw evidence. Authorization is resolved by the evidence service.

## Payable duration

The approved quantity eligible for worker compensation. It may differ from
customer-billable duration and must not be inferred from sensors alone.

## Projection

A deterministic, rebuildable record derived from source events and a versioned
policy or algorithm, such as an approved time entry or invoice draft.

## Provenance

The source and transformation history of a record: who or what produced it,
versions, timestamps, source IDs, reviews, approvals, and revisions.

## Raw duration

The union of the underlying candidate intervals. It preserves what the inference
proposed even when a human corrects the approved interval.

## Raw evidence

High-risk source media or content such as audio, photos, receipts, transcripts, or
precise location traces. It requires stronger encryption, access, and retention
controls than derived events.

## Redaction

A controlled process that removes or restricts content while preserving an
explicit audit/availability transition. Redaction is different from rejecting a
candidate time span.

## Report draft

A versioned, editable report that contains grounded statements and selected
evidence. It is not externally final until approved/frozen under the relevant
workflow.

## Review burden

The time and number of actions required for a human to turn candidates into
approved records. It is a core product and model-quality metric.

## Rolling buffer

A short local audio window that can support cue recognition or user-selected clips
without requiring permanent retention of the entire session.

## Semantic validator

Code that enforces cross-record rules that JSON Schema alone cannot express, such
as valid lineage, report grounding, non-billable leakage, deterministic totals,
and duplicate source billing.

## Selective synchronization

Synchronization of approved or policy-permitted records/evidence rather than
unrestricted upload of everything captured on the device.

## Sister app

The separate contractor work-intelligence product that may reuse versioned Sonus
capabilities but owns its own business domain, production data, credentials,
releases, and customer experience.

## Source packet

The purpose-limited set of structured records provided to a report generator. It
excludes unrelated tenant data and unselected raw evidence.

## Supersession

An append-only relationship where a newer candidate, review, report, invoice, or
policy revision replaces the active meaning of an earlier record without deleting
history.

## Tenant

The organization-level isolation boundary. Every durable record and object access
must be authorized within a tenant and, where relevant, a job/session.

## Verbal cue

A configured spoken control or note class, ideally processed on device. Cues that
change job or billing state require confirmation or review.

## Worker-controlled capture

Capture that a worker can see, start or accept, pause, stop, inspect, and understand.
It excludes silent remote microphone activation.

## Working category

A provisional product description used during discovery. “Contractor Work
Intelligence” must not be treated as the final brand before DEN-990 is resolved.