# Product brief

## Status and naming

**Contractor Work Intelligence** is a working category for a separate Sonus Auris
sister product. It is not the final company, application, package, or repository
name. Linear DEN-990 must select the launch trade, economic buyer, pricing model,
and brand before public launch assets or a final GitHub organization are created.

## One-sentence promise

Help contractors reconstruct less work from memory by turning a visibly enabled,
job-scoped activity timeline into reviewable time records, daily reports,
customer updates, change-order evidence, and invoice drafts.

## The problem

Small contractors and field-service crews often finish technical work before they
finish documenting it. Time and materials are reconstructed after the fact from
memory, calendars, messages, receipts, photos, GPS traces, and incomplete notes.
That creates several recurring costs:

- billable labor, travel, materials, and exceptions are forgotten;
- workers spend evenings rebuilding timesheets and reports;
- customer updates are inconsistent or delayed;
- change-order evidence is scattered;
- managers cannot distinguish a model guess from a worker-confirmed fact;
- disputes are harder to resolve because source evidence and edit history are
  missing;
- conventional monitoring products damage worker trust by collecting too much
  data or turning weak signals into productivity judgments.

The sister app should reduce administrative reconstruction without becoming a
covert surveillance or automated wage-control system.

## Target users

### Primary launch candidates

The first pilot should select one narrow field-service vertical after discovery.
Good candidates share short-to-medium jobs, recurring documentation burden, and a
clear worker-review moment before billing. Examples include:

- plumbing, electrical, HVAC, appliance, and equipment repair;
- property maintenance and inspection;
- cleaning, restoration, pressure washing, and landscaping;
- installation, low-voltage, and light construction subcontracting;
- mobile mechanics and specialized service technicians.

### Personas

#### Independent contractor or owner-operator

Needs to capture what happened while hands are occupied, approve a clean timeline,
and turn it into a report or invoice without reconstructing the day at night.
This user is often both worker and final approver.

#### Field technician or subcontractor

Needs visible control over capture, a trustworthy personal record, a fast review
experience, and assurance that ambient audio is not silently converted into
payroll, discipline, or performance scoring.

#### Crew lead or operations manager

Needs job status, approved time, exceptions, materials, and source-backed reports
without unrestricted access to workers' raw recordings or private off-job data.

#### Bookkeeper or billing administrator

Needs deterministic quantities, rate versions, approval lineage, exports, and a
clear distinction between drafts and issued accounting records.

#### Customer or general contractor

May receive a selected report, completion summary, change-order packet, or invoice
revision. This role must never receive the worker's complete raw timeline by
default.

## Jobs to be done

1. **Start a trustworthy job record.** Select the job, see exactly which sensors
   are active, confirm policy, and start or schedule a job session.
2. **Capture context without stopping work.** Add verbal notes, photos, material
   counts, measurements, task changes, problems, and completion cues.
3. **Reconstruct the timeline.** Combine explicit controls and corroborating
   signals into candidate activity spans with confidence and provenance.
4. **Correct before consequences.** Merge, split, relabel, redact, reject, or
   approve candidate events and time intervals.
5. **Create business records.** Produce approved timesheets, field reports,
   customer summaries, change-order drafts, and invoice drafts.
6. **Explain every result.** Show the evidence, inference, correction, approval,
   rate policy, and calculation version behind each important field.
7. **Share selectively.** Send only approved revisions and selected evidence.
8. **Retain or delete intentionally.** Apply job, tenant, worker, and evidence-type
   retention policies with auditable redaction or deletion events.

## Product principles

### 1. Visible, job-scoped capture

Capture begins through an explicit worker action or an unambiguous schedule that
still produces a visible notification and simple pause/stop control. The product
must never default to hidden workplace recording.

### 2. Evidence before inference; approval before consequence

Manual actions and sensor observations enter an append-only evidence ledger.
Rules or models create candidate spans. Humans approve accounting and external
claims. This ordering is a product invariant, not merely a UI preference.

### 3. Local-first and useful offline

A worker must be able to start, capture, annotate, review, and preserve a job
session without network connectivity. Synchronization may be delayed; job capture
must not be blocked by a cloud outage.

### 4. Minimal collection and selective sharing

Derived events are preferred over raw media. Raw clips are retained or uploaded
only under explicit policy. Manager and customer views are purpose-limited.

### 5. Deterministic business arithmetic

Models may draft descriptions and suggest categories. They do not calculate
money, taxes, payroll, or rate application. Those outcomes use versioned,
testable rules.

### 6. Corrections improve the system without rewriting history

A correction creates a new review event and projection. It does not erase the
original observation or model proposal. Training labels must preserve who changed
what and why without exposing unnecessary raw content.

### 7. Worker trust is a launch metric

Pause behavior, deletion use, review corrections, opt-outs, and qualitative trust
feedback are core product outcomes, not compliance afterthoughts.

## MVP scope

The first pilot should support:

- organization, worker, customer, site, job, assignment, and rate-card setup;
- visible manual and verbal job start, pause, resume, and stop;
- local encrypted event storage;
- timestamped photo and voice/text notes;
- a deliberately small opt-in cue vocabulary;
- one or two corroborating non-speech sound classes relevant to the pilot trade;
- candidate timeline generation with provenance and confidence;
- fast merge, split, relabel, reject, redact, and approve actions;
- approved billable and non-billable time entries;
- one daily field-report template;
- one hourly/fixed-price invoice-draft template;
- PDF, CSV, and JSON export of approved records;
- explicit consent, retention, deletion, and sharing controls;
- complete offline capture and eventual synchronization.

## Explicit non-goals for the first release

- automatic payroll submission;
- automatic customer billing or invoice issuance;
- automatic wage deductions, unpaid-break decisions, or overtime denial;
- worker ranking, attitude analysis, or productivity scoring from audio;
- speaker identification or emotion inference;
- unrestricted manager access to raw recordings;
- continuous background capture outside an active job policy;
- broad accounting, payment, ERP, or payroll integrations;
- dozens of trade-specific acoustic classifiers;
- legal, tax, safety, licensing, or contract-compliance guarantees;
- replacing project management, dispatch, CRM, or full accounting systems.

## Business model hypotheses

Pricing remains a discovery question. Candidate models include:

- per active worker per month;
- per organization with included worker tiers;
- usage-based report, storage, or transcription allowances;
- premium compliance, retention, or integration packages;
- solo-contractor plan plus crew/business plan.

The product should not rely on selling or exploiting worker data. Storage-heavy raw
audio features must have transparent cost and retention controls.

## Success metrics

### User value

- median minutes saved per completed job report;
- median minutes saved per approved invoice draft;
- recovered billable labor or materials that the user confirms would otherwise
  have been omitted;
- percentage of job sessions reviewed the same day;
- report and invoice fields accepted without modification;
- time from job completion to approved report and invoice draft.

### Model and workflow quality

- correction rate by activity category and cue source;
- false positive and false negative rates for each opt-in cue;
- percentage of candidate time approved, corrected, or rejected;
- review actions per hour of captured work;
- confidence calibration, not only top-line accuracy;
- rate of customer-facing statements rejected because grounding is insufficient.

### Trust and reliability

- capture pauses and opt-outs, interpreted with user interviews rather than as
  automatically negative behavior;
- deletion/redaction completion time and failure rate;
- raw-media upload percentage versus derived-event-only use;
- battery, storage, crash-free session, and offline recovery metrics;
- customer dispute rate and usefulness of selected evidence;
- worker-reported trust and clarity of recording state.

## Launch gates

A pilot must not begin until:

- the launch vertical and buyer are selected with interview evidence;
- capture and sharing policies are understandable in a five-minute onboarding;
- offline start/stop and crash recovery are reliable on representative devices;
- candidate events cannot bypass review into reports, payroll, or invoices;
- deterministic money calculations have golden vectors across supported runtimes;
- tenant, job, and customer isolation tests pass;
- raw-media retention and deletion tests pass;
- a worker can inspect their own event, correction, approval, and export history;
- support can diagnose failures without receiving raw audio by default.

## Definition of product-market evidence

The first pilot is informative when a meaningful cohort repeatedly uses the
product on real jobs and demonstrates:

1. material reduction in after-hours administrative work;
2. acceptable correction burden;
3. recovered or better-documented revenue without inflated billing;
4. worker willingness to keep visible capture enabled;
5. customer or manager value from grounded reports;
6. sustainable battery, storage, support, and inference cost.

A technically functioning classifier or invoice screen is not, by itself,
product validation.