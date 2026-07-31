# Operations and quality

## Quality objective

The product must be more reliable and explainable than reconstructing a workday
from memory, while failing safely when sensors, models, synchronization, or cloud
services are unavailable. Reliability does not mean pretending uncertain evidence
is certain.

## Test strategy

### Contract tests

Canonical schemas and semantic validators must verify:

- structural validity and explicit contract versions;
- tenant/job/session scope;
- provenance and idempotency identities;
- evidence-reference integrity;
- observation → candidate → review → approved-time lineage;
- report grounding;
- deterministic money calculations;
- revision and supersession rules;
- privacy-sensitive payload restrictions.

Every contract release includes positive golden fixtures and negative fixtures for
forbidden state transitions.

### Pure-domain unit tests

Rust domain modules should use table-driven and property-based tests for:

- activity-review state transitions;
- approved-time projections;
- interval merge/split/overlap behavior;
- rate selection and effective dates;
- rounding and integer overflow boundaries;
- revision/idempotency keys;
- retention-deadline calculations;
- authorization decisions;
- report-source filtering.

Business arithmetic should be testable without a database, network, or model.

### Client tests

Flutter/Dart tests should cover:

- permission and onboarding state;
- capture state visibility;
- offline job start/stop;
- sensor-degraded states;
- timeline review operations;
- optimistic local writes and sync acknowledgements;
- report/invoice source explanations;
- privacy controls and selective sharing;
- accessibility semantics.

Native integration tests are required for background execution, microphone
interruptions, phone calls, Bluetooth device changes, battery restrictions, and
platform retention behavior.

### API tests

API tests should cover:

- authentication and trusted-device flows;
- cross-tenant and cross-job identifier probing;
- idempotent append and projection requests;
- pagination and audit ordering;
- optimistic revision conflicts;
- signed-share scope and revocation;
- evidence-object authorization;
- rate-policy absence and version mismatch;
- deletion/redaction workflows;
- webhook/export replay.

### End-to-end job scenarios

Minimum E2E matrix:

1. fully offline job, review, reconnect, and sync;
2. app crash during active capture and conservative recovery;
3. microphone unplug/reconnect during a job;
4. explicit break plus false acoustic candidate;
5. human interval correction beyond the model proposal;
6. manager/worker concurrent review conflict;
7. report generation with grounded and ungrounded statements;
8. deterministic invoice regeneration and duplicate prevention;
9. selected evidence sharing and revocation;
10. retention expiry while a report still references evidence metadata;
11. account/device loss and recovery;
12. customer portal access attempting cross-revision/cross-tenant traversal.

## Model evaluation

Each cue or inference model needs an evaluation card containing:

- intended purpose and prohibited uses;
- input features and data classes;
- supported devices/platforms/trades;
- training/evaluation dataset provenance and consent;
- version and configuration hash;
- precision, recall, false-positive/negative rates;
- confidence calibration;
- duration-boundary error;
- subgroup/environment robustness where legally and ethically appropriate;
- battery/CPU/storage cost;
- degradation behavior;
- rollback/disable plan.

### Pilot evaluation metrics

- worker approval/correction/rejection rate by source and category;
- median start/end boundary error;
- false cue activations per job hour;
- missed explicit cues;
- review time per captured hour;
- percentage of sessions needing manual-only reconstruction;
- worker trust and qualitative explanations for pauses/opt-outs.

Accuracy must not be optimized by collecting broader ambient content than the
product purpose requires.

## CI gates

A PR that changes the sister-product contract, shared capture interface, report
logic, billing logic, or privacy behavior should run:

- Markdown link and formatting checks;
- JSON Schema meta-validation;
- generated-artifact drift check;
- Rust format, check, Clippy with warnings denied, and tests;
- Dart format, analyze, and tests;
- Node contract/semantic tests;
- property/golden calculation vectors;
- secret and credential scanning;
- dependency and license review;
- API compatibility checks;
- privacy payload/log scanning;
- E2E subset for affected workflows.

Required checks must run on the exact final head. A workflow that fails before
runner allocation is an infrastructure failure, not a passing or failing test.
Do not merge by mislabeling unexecuted checks as green.

## Documentation gates

A material change is incomplete unless documentation is updated in the same PR or
a blocking documentation issue is linked.

Checks should verify:

- every handbook link resolves;
- the documented contract version exists;
- Mermaid diagrams parse where a checker is available;
- API examples validate against schemas;
- money examples match calculation vectors;
- no final product name is asserted before DEN-990;
- implemented, accepted, hypothetical, and future behavior are distinguishable;
- prohibited surveillance behaviors remain explicit.

## Observability

### Principles

- instrument state transitions and latency, not raw content;
- use pseudonymous tenant/job/session/device identifiers;
- correlate local and server events with trace IDs without embedding evidence;
- separate business/audit records from operational telemetry;
- provide privacy-safe support diagnostics;
- make dropped, delayed, duplicated, and rejected events visible.

### Core metrics

#### Device

- session-start success and latency;
- capture continuity and gap duration;
- microphone/device changes;
- local write failures;
- retention-sweep success/failure;
- battery/CPU/storage consumption;
- offline queue depth and age;
- sync retries and conflicts;
- crash-free active-session rate.

#### API and sync

- append latency and idempotency hits/conflicts;
- authorization denials by safe reason code;
- per-tenant queue lag without content;
- projection rebuild latency;
- conflict counts;
- evidence upload/download failures;
- deletion/redaction completion latency;
- export/webhook delivery and replay.

#### Inference

- model/rule version distribution;
- candidate volume by category/source;
- confidence calibration aggregates;
- approval/correction/rejection rates;
- inference latency and backlog;
- disabled/rolled-back model versions.

#### Reports and billing

- draft generation latency/failure;
- unsupported-statement rejection count;
- human edit rate;
- deterministic mismatch failures;
- missing-rate-policy blocks;
- duplicate-source billing attempts;
- revision counts and approval latency.

## Logs

Allowed examples:

```text
job_session_start_failed code=MIC_PERMISSION_DENIED platform=android
sync_append_idempotent tenant_ref=t_7f2 event_type=activity_review
invoice_projection_blocked code=MISSING_RATE_POLICY version=deterministic-labor-v2
```

Forbidden examples:

```text
transcript="customer said..."
raw_audio_base64=...
receipt_ocr_full_text=...
signed_url=https://...
access_token=...
customer_address="..."
```

Log fields should be allow-listed, not derived from arbitrary payload serialization.

## Audit history

The audit ledger records consequence-bearing actions:

- capture-policy changes;
- job-session lifecycle;
- evidence selection/redaction/deletion;
- review approval/rejection/correction;
- manager supersession;
- rate-card and policy changes;
- report/invoice approval, freeze, delivery, withdrawal, issue, or void;
- customer share creation/revocation/access where permitted;
- export and integration delivery;
- support break-glass access;
- model version activation/rollback when it affects candidate generation.

Audit records are content-minimized but sufficient to identify actor, action,
scope, revision, time, and policy/version.

## Reliability objectives

Pilot objectives should be measured before contractual SLOs are offered.
Reasonable engineering targets include:

- 99.9% successful durable local job-session starts on supported devices when
  storage and required permissions are available;
- zero acknowledged local events lost across a recoverable app restart;
- 99% of synchronized append batches accepted or idempotently recognized within
  one minute after stable connectivity returns;
- report/invoice generation retries without duplicate revisions;
- no known cross-tenant data exposure;
- retention/deletion workflows with explicit completion or actionable failure;
- graceful manual operation during inference/report-model outages.

Cloud availability cannot compensate for unreliable local capture.

## Incident priorities

### Severity 0 / launch stop

- covert or remotely activated capture;
- cross-tenant raw evidence or business-record exposure;
- candidate/model output directly causing payroll, invoice issue, or discipline;
- deletion falsely reported complete;
- deterministic billing silently producing wrong totals;
- worker unable to stop visible capture;
- credentials or raw evidence emitted to ordinary logs.

### Severity 1

- durable approved records lost or corrupted;
- customer share exposing unintended evidence;
- widespread job-session start failure;
- retention worker failure risking policy breach;
- idempotency failure causing duplicate billing/export;
- authorization bypass within a tenant.

### Severity 2

- delayed inference/report generation with manual fallback;
- isolated sync backlog;
- inaccurate candidate classifications that remain review-only;
- noncritical export adapter failure.

## Incident response requirements

- preserve relevant audit and version metadata;
- disable or roll back the affected model/feature independently;
- prevent further sharing or billing consequence;
- notify affected users according to policy and law;
- provide worker/customer correction or withdrawal paths;
- avoid copying raw evidence into incident tickets;
- document root cause, affected contract/policy versions, and regression tests.

## Security testing

- authorization matrix and IDOR testing;
- tenant isolation property tests;
- customer-link token entropy, expiry, and revocation;
- device-key theft and replay scenarios;
- sync-envelope tampering and duplicate delivery;
- prompt injection from notes, transcripts, receipts, and imported documents;
- malicious file and metadata handling;
- object-store locator and signed-URL leakage tests;
- dependency/supply-chain review;
- secret scanning and least-privilege cloud roles;
- backup/restore isolation and deletion propagation;
- support break-glass audit testing.

## Pilot rollout

### Internal dogfood

Use synthetic and consenting team-owned jobs. Validate state machines, offline
behavior, review burden, and privacy controls before involving contractors.

### Design partners

A small number of owner-operators where the same person controls capture and
billing. Avoid employer/employee deployments until worker-rights and dispute
workflows are proven.

### Small crew pilot

Add worker/reviewer separation, manager corrections, customer sharing, and role
boundaries. Conduct worker interviews independently of management.

### Limited availability

Only after reliability, trust, correction burden, and support costs meet documented
gates. Keep model and integration scope narrow.

## Pilot go/no-go gates

Go only when:

- users understand capture state and data sharing;
- offline capture and recovery pass field tests;
- candidate approval cannot be bypassed;
- review burden is acceptable for the pilot trade;
- report grounding and billing vectors are green;
- privacy deletion/redaction tests pass;
- worker and manager permissions are validated;
- incident and support procedures are rehearsed;
- final head CI actually executes.

No-go conditions include:

- pressure to hide or remotely activate capture;
- customer demand for worker scoring from ambient signals;
- unmanageable correction rates;
- battery/storage costs that cause workers to disable the app;
- inability to comply with pilot-site consent or labor requirements;
- unresolved cross-tenant or billing-integrity defects.

## Definition of done for a feature

A feature is done when:

1. product behavior and non-goals are documented;
2. domain states and invariants are explicit;
3. contract/API changes are versioned;
4. privacy collection, visibility, retention, and deletion are defined;
5. offline and failure behavior is implemented;
6. unit, contract, integration, and relevant E2E tests pass;
7. telemetry is content-minimized;
8. authorization and tenant isolation are tested;
9. support and incident behavior are documented;
10. the exact merged head is verified in GitHub and linked from Linear.