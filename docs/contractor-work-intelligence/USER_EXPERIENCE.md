# User experience

## Experience goals

The app should feel like a worker-controlled field notebook that happens to be
excellent at reconstructing time and producing business records. It must not feel
like a hidden monitoring agent.

The primary experience goals are:

- capture state is obvious at a glance;
- starting and stopping a job is faster than opening a paper timesheet;
- verbal and sonic cues reduce interruption but never remove review;
- the end-of-job review is short, explainable, and reversible;
- generated reports and invoices show where each fact or quantity came from;
- offline and failure states are honest;
- workers retain access to their own history and corrections.

## Navigation model

A pilot application can use five primary destinations:

1. **Today** — scheduled jobs, active session, sync state, and pending reviews.
2. **Jobs** — customers, sites, work orders, assignments, and history.
3. **Review** — candidate timeline, unresolved conflicts, reports, and invoices.
4. **Evidence** — user-selected notes, photos, receipts, and retained clips.
5. **Settings** — capture policy, cue vocabulary, retention, sharing, devices,
   privacy, exports, and account controls.

Manager and billing roles may add **Crew** and **Billing**, but the worker's active
capture state must remain accessible from every screen.

## Onboarding

### Required disclosures

Before the first job session, onboarding must explain:

- what the microphone, location, motion, and other permissions are used for;
- which processing occurs on device;
- whether raw audio is retained, for how long, and where;
- which derived events may synchronize;
- who can see raw evidence, derived events, approved time, and shared reports;
- how to pause, stop, redact, delete, and export;
- that model suggestions cannot automatically become payroll, discipline, or
  invoices;
- applicable business policy and a jurisdictional-consent warning.

Consent must be granular enough to permit a non-audio mode where the product can
still provide value.

### Device readiness check

The onboarding flow should test:

- microphone permission and input selection;
- notification permission;
- local encrypted storage availability;
- background-execution capability;
- battery-optimization restrictions;
- optional location/motion permission;
- network-independent session creation;
- device key and account recovery path.

The result is a readable checklist, not a silent pass/fail gate.

## Job setup

A job card should contain:

- customer and site;
- work-order or contract reference;
- assigned worker/crew;
- expected time window;
- permitted sensors and retention policy;
- active rate-card version;
- job notes and access restrictions;
- fixed-price milestones or approval requirements;
- offline availability indicator.

The app must cache enough job and policy data to start offline. If the policy is
stale, the UI shows the version and the last synchronization time.

## Starting a job session

The start sheet must show:

- selected job and site;
- current time and timezone;
- sensor status chips;
- raw-audio retention status;
- connected-device status;
- capture-policy version;
- a clear **Start job** action;
- an alternative **Start without audio** action when policy permits;
- any unresolved consent or site restrictions.

After start, the app provides persistent visible state through:

- an in-app recording banner;
- OS notification/background indicator;
- elapsed session time;
- current job name;
- pause and stop actions;
- sensor-degraded warnings;
- offline and sync state.

A successful start means the local ledger durably recorded the session-start
event. It does not depend on a server response.

## Active-session controls

Primary controls should be reachable with one hand and support gloves or noisy
sites:

- pause/resume;
- add note;
- take photo;
- scan receipt/material/QR/NFC;
- mark travel, material run, break, customer discussion, problem, or completion;
- select a short evidence clip from the rolling buffer;
- stop job.

Voice controls are configurable and confirmation-sensitive. Examples:

- “start the Anderson job”;
- “taking lunch”;
- “resume work”;
- “materials run”;
- “add two replacement valves”;
- “customer approved the repair”;
- “job complete.”

A verbal cue that changes billing state should use a brief tone, haptic response,
or visible confirmation. Ambiguous cues create a review item instead of silently
changing the session.

## Sensor and permission degradation

The active screen distinguishes:

- session active;
- microphone capturing;
- cue recognition running;
- raw audio retained or derived-events-only;
- location available;
- motion available;
- local storage healthy;
- sync online/offline.

Examples:

- **Microphone disconnected:** “Job session continues. Audio cues and clips are
  unavailable until the microphone reconnects.”
- **Storage low:** pause raw evidence first, preserve content-minimized events, and
  show the exact consequence.
- **Permission revoked:** do not loop permission prompts; show how to continue in
  reduced mode or stop.
- **Background execution restricted:** display setup guidance and mark gaps rather
  than implying continuous capture.

## Candidate timeline

The timeline visually distinguishes record classes:

| Visual type | Meaning |
| --- | --- |
| Solid manual marker | explicit worker action |
| Evidence marker | photo, note, receipt, selected clip, or imported source |
| Dashed candidate span | rule/model proposal awaiting review |
| Solid approved span | human-approved time/work fact |
| Struck/rejected item | preserved rejected proposal |
| Warning band | sensor gap, clock uncertainty, or sync conflict |

Each candidate card shows:

- proposed category and time range;
- billability suggestion, if any;
- confidence and alternatives;
- source observations;
- model/rule name and version in an expandable details panel;
- approve, correct, split, merge, or reject actions.

Confidence should not be represented as a false precision meter. Use language such
as “strong signal” with the numeric value available in details for expert review.

## End-of-job review

When the worker stops a session, the app should summarize:

- total observed session span;
- candidate billable and non-billable time;
- gaps or overlaps;
- unreviewed notes/materials;
- sensor outages;
- unresolved conflicts;
- report and invoice readiness.

The default review order is risk-based:

1. gaps and overlaps;
2. billability and break decisions;
3. material/expense quantities;
4. customer decisions and change-order candidates;
5. report narrative;
6. invoice calculations.

Bulk approval is permitted only for low-risk items that meet documented policy.
Every bulk action must preview the affected records and remain reversible through a
superseding review event.

## Correction patterns

### Split

Divide a candidate interval into two or more intervals. Preserve the original
candidate ID as lineage for every resulting approval.

### Merge

Combine adjacent compatible candidate intervals. Preserve all source candidate
and observation IDs.

### Relabel

Change category or billability while preserving the original proposal.

### Trim or extend

Adjust start/end. Display both raw proposed duration and approved duration.

### Reject

Require a reason only when policy needs one; do not make workers justify every
model mistake with free text.

### Redact

Remove or restrict evidence content while preserving an audit-safe availability
transition. Redaction must not be conflated with rejecting a time candidate.

## Reports experience

The report editor presents:

- template and intended audience;
- sections and generated statements;
- source badges for each statement;
- warnings for unsupported or weakly grounded statements;
- selected photos/clips/documents;
- edit history;
- draft/approved/frozen/delivered state.

Editing generated prose must not detach it silently from sources. The system may:

- retain existing source links when the meaning remains supported;
- ask the user to select a source;
- mark the statement as user-authored;
- block external approval if a required claim has no approved source.

## Invoice experience

The invoice editor shows:

- source approved-time/material/expense record;
- quantity and unit;
- rate-card policy/version;
- deterministic calculation explanation;
- subtotal, tax, discount, credit, retainage, and total;
- warnings for missing policy or unsupported jurisdiction;
- revision and issue state.

A user can change an invoice only through an explicit business action:

- change billable quantity within authorized limits;
- select a different approved rate policy;
- add an approved material/expense record;
- add a documented manual adjustment;
- create a new revision.

The interface never displays an LLM confidence for monetary arithmetic because the
arithmetic is deterministic.

## Customer sharing

Before sharing, show exactly:

- report/invoice revision;
- selected evidence;
- recipients and expiration;
- whether download is allowed;
- what is excluded;
- revocation behavior;
- an immutable preview.

The customer portal receives only selected approved revisions and evidence grants.
It must not expose the full worker timeline, model confidence history, other jobs,
or raw session audio.

## Worker access and disputes

Workers can view and export:

- their observations;
- candidate spans;
- review events and corrections;
- approved time;
- manager changes and superseding decisions;
- report/invoice lineage relevant to their work;
- capture and retention policy versions;
- sharing/export history.

A dispute action should freeze the contested projection, preserve evidence under
the disclosed policy, and route the record to an authorized reviewer. It must not
silently alter payroll or invoicing.

## Notifications

High-value notifications include:

- scheduled job available offline;
- job session still active after expected end;
- microphone/capture degraded;
- session recovered after crash;
- review required;
- manager correction requires acknowledgment;
- report/invoice approved or returned;
- retention deadline approaching for selected evidence;
- customer viewed or downloaded a shared revision;
- deletion/redaction completed or failed.

Avoid noisy notifications for every model classification.

## Accessibility and field ergonomics

- large touch targets and high contrast;
- screen-reader labels for capture and sensor state;
- haptic and visual alternatives to audio confirmations;
- localization-ready date, duration, currency, and terminology;
- no color-only distinction between candidate and approved records;
- one-handed and landscape support;
- sunlight-readable active-session screen;
- optional simplified mode for solo contractors;
- keyboard navigation for desktop review/billing.

## Empty and error states

Error messages must describe the business consequence:

Bad: “Inference worker 503.”

Good: “Your observations are saved. Automatic timeline suggestions are delayed;
you can review manually or generate suggestions when the service recovers.”

Bad: “Upload failed.”

Good: “The selected clip remains encrypted on this device and has not been shared.
Retry when online or remove it from the report.”

## UX acceptance scenarios

1. Start and finish a complete offline job, then synchronize and review without
   duplicate events.
2. Continue a session after microphone failure with accurate degraded-state UI.
3. Reject a false tool-sound candidate and verify no report or invoice consequence.
4. Correct an interval beyond the model proposal and see raw versus approved time.
5. Approve a non-billable break and verify zero billable quantity.
6. Edit a report sentence and preserve or reselect grounding.
7. Generate the same invoice twice and receive the same idempotent draft result.
8. Share one report photo without sharing the rest of the evidence timeline.
9. Redact a clip and see the availability state propagate to every view.
10. Export a worker history that explains manager corrections and approvals.