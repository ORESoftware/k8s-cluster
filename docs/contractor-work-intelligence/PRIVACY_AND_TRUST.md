# Privacy and trust

## Policy status

This document defines mandatory product and engineering guardrails for the
contractor sister app. It is not jurisdiction-specific legal advice. Product
launch requires review for the pilot locations, trades, employment relationships,
contracts, recording-consent rules, biometric restrictions, wage/hour rules, and
customer-site requirements.

## Trust promise

The product is a worker-controlled documentation and billing assistant. It is not
a covert employee-surveillance product.

The app must make it easy to answer:

- Is a job session active?
- Is the microphone active?
- Is raw audio retained, and until when?
- Which derived events are being created?
- Who can see each class of data?
- What has been shared?
- How can the worker pause, stop, redact, delete, export, or dispute a record?
- Which facts were inferred, corrected, approved, or generated?

## Prohibited uses

The platform must not provide, market, or quietly enable:

- covert workplace or customer recording;
- ambient-conversation monitoring outside a visible job-scoped session;
- worker attitude, emotion, honesty, protected-characteristic, or personality
  inference;
- speaker identification for workforce scoring;
- productivity ranking from silence, tool noise, location, or model classifications;
- automatic unpaid-break, wage-deduction, overtime-denial, discipline, termination,
  contract-breach, or fraud decisions from sensor/model data;
- automatic customer billing from unreviewed observations or candidates;
- unrestricted manager playback of a worker's day;
- use of raw evidence for model training without separate informed opt-in;
- selling or advertising against worker or customer activity data.

These are architecture constraints. They cannot be overridden through a hidden
feature flag or customer configuration.

## Consent model

Consent is layered rather than a single blanket checkbox.

### Account and policy acknowledgment

The worker acknowledges the organization policy, intended uses, role access,
retention, and dispute process. The exact policy version is recorded.

### Device permissions

Microphone, notifications, location, motion, camera, photos, and background
execution are requested separately and only when needed.

### Job-session consent

Before each session or accepted schedule, the worker sees the job, site, active
sensors, raw-media behavior, and sharing policy. The worker can start, start in a
reduced mode when allowed, or decline.

### Evidence selection

Selecting a raw clip, photo, receipt, or transcript excerpt for retention/sharing
is a separate action from permitting local rolling capture.

### Training consent

Model-training contribution is separate, optional, revocable for future use, and
must explain what content or derived labels leave the operational system.

## Recording visibility

While microphone capture or an audio rolling buffer is active, the app must use:

- a persistent in-app banner;
- an operating-system notification or platform recording indicator;
- clear elapsed time and current job;
- one-step pause and stop controls;
- explicit sensor-degraded state;
- no misleading “session active” indicator that hides whether audio is active.

A manager cannot remotely start a microphone without the worker's explicit action
on the device. Scheduled starts require a visible notification and must follow
platform and jurisdiction requirements.

## Collection minimization

Preferred hierarchy:

1. explicit manual action;
2. content-minimized verbal cue class;
3. derived non-speech acoustic class;
4. selected transcript/note;
5. selected short evidence clip;
6. full-session raw recording only under a separate, disclosed policy.

When a derived event is sufficient, do not upload raw media. A classification such
as `powerDrill: 0.91` is still sensitive work metadata, but carries less exposure
than an unrestricted audio file.

## Raw audio policy

Raw audio is classified as restricted data.

Default requirements:

- encrypted on device;
- segmented and deadline-enforced;
- local-first;
- short configurable retention;
- no background cloud upload unless explicitly enabled;
- selective clip sharing rather than whole-session sharing;
- separate encryption keys from ordinary report and billing services;
- no raw audio in database rows, logs, analytics events, support bundles, crash
  reports, or model-evaluation telemetry;
- deletion and cryptographic-erasure behavior tested under interruption.

The Sonus Auris 100-hour plaintext boundary is a capability ceiling, not a required
contractor default. The sister app should generally use much shorter raw-media
retention unless the worker selects evidence or policy requires otherwise.

## Conversation and transcript policy

- Voice activity may be detected without retaining speech content.
- Configured job-control cues should be processed on device when practical.
- Transcript generation is opt-in, job-scoped, and purpose-limited.
- Customer or coworker speech must not be attributed to an identity without an
  explicit supported workflow and legal review.
- Reports should use worker-confirmed summaries rather than copying ambient
  conversation.
- Sensitive transcript portions can be redacted independently of time records.
- Search indexes and embeddings follow the same retention and deletion policy as
  the source content.

## Location and motion policy

Location and motion are optional corroborating signals, not proof of labor.

Requirements:

- collect at the lowest precision and frequency that meets the job workflow;
- avoid off-job tracking;
- clearly show whether a geofence or continuous location mode is active;
- do not infer unpaid time or discipline from leaving a site;
- preserve uncertainty and sensor gaps;
- separate precise capture location from customer-facing site address;
- provide a non-location workflow when feasible;
- delete raw traces earlier than approved business summaries unless explicitly
  retained.

## Role-based visibility

| Record | Worker | Reviewer/manager | Billing admin | Customer |
| --- | --- | --- | --- | --- |
| Own raw observations | Yes | Derived metadata by default | No | No |
| Raw audio/photo evidence | Yes | Only selected/policy-authorized | Only selected when needed | Only explicitly shared |
| Candidate spans/confidence | Yes | When authorized to review | Usually no | No |
| Review events and corrections | Yes | Yes for authorized jobs | Relevant lineage | No |
| Approved time | Yes | Yes | Yes | Selected summary only |
| Internal report draft | Yes/role policy | Yes | Relevant | No |
| Frozen customer report | Yes | Yes | Yes | Explicit revision only |
| Invoice draft | Relevant worker lineage | Role policy | Yes | Only issued/shared revision |

Support access is not included in ordinary roles. It uses audited break-glass
controls and defaults to metadata only.

## Data classification

### Restricted

- raw audio and video;
- photos/receipts containing personal or payment data;
- transcript content;
- precise location history;
- credentials, encryption keys, signed storage URLs.

### Sensitive

- derived acoustic classes;
- customer decisions and incident notes;
- candidate timelines and model confidence;
- device and job linkage;
- worker correction and dispute history.

### Business-confidential

- approved time;
- rate cards;
- reports, change orders, invoice drafts;
- contracts and customer/job metadata.

### Operational

- service health;
- content-minimized model/version telemetry;
- sync cursors;
- error codes and aggregate performance.

## Retention model

Retention is configured by data class and purpose, not one global duration.

| Data | Default direction |
| --- | --- |
| Rolling raw buffer | Minutes to hours; local only |
| Unselected full raw segments | Shortest practical period |
| Selected evidence clips | Job/policy-specific |
| Derived observations | Longer, content-minimized |
| Rejected candidate spans | Retain for audit/evaluation under policy |
| Approved time and business records | Business/legal retention policy |
| Search indexes/embeddings | No longer than source |
| Support diagnostics | Short, metadata-only |
| Model-training dataset | Separate consent and governance |

Retention deadlines must be immutable or conservatively shortened. Clock rollback
must not extend plaintext deadlines.

## Redaction, deletion, and erasure

The user interface must distinguish:

- **remove from report** — stop presenting evidence in a report revision;
- **revoke share** — withdraw recipient access where technically possible;
- **redact content** — create a replacement/derivative and mark the original
  unavailable to ordinary use;
- **delete artifact** — remove stored object and indexes;
- **cryptographically erase** — destroy applicable key material;
- **delete account/job data** — execute a scoped policy workflow;
- **preserve required record** — explain why minimum accounting/audit data remains.

Completion is reported only after all defined stores, indexes, caches, replicas,
exports, and key states reach the documented terminal state or an explicit
exception is recorded.

## Sharing grants

Customer or third-party sharing is explicit and revision-specific.

A grant includes:

- recipient;
- report/invoice revision;
- selected evidence IDs;
- created/expiry time;
- download permission;
- revocation status;
- audit trail.

A customer link cannot traverse to the worker timeline or organization data.
Opaque tokens must be short-lived or revocable, scoped, and absent from logs.

## Worker rights and transparency

The product should provide workers with:

- access to their observations, candidates, reviews, approved time, and export
  history;
- visibility into manager corrections and superseding approvals;
- policy version and sensor state history;
- correction and dispute controls;
- export of their own records in a portable format;
- understandable deletion/redaction behavior;
- a contact/escalation path.

A business configuration that removes these rights requires explicit product,
legal, and trust review; it cannot be an undocumented enterprise toggle.

## Model evaluation governance

Operational inference evaluation uses content-minimized records where possible:

- source type and model version;
- predicted category and confidence;
- human decision and correction delta;
- duration and timing error;
- device/platform/trade cohort at an aggregated level;
- no raw transcript/audio in ordinary telemetry.

Raw examples for debugging or training require a separate evidence-selection and
consent workflow. Access is time-bounded and audited.

## Logging and support

Normal logs must exclude:

- raw audio, transcript text, photo/receipt content;
- precise address/location trace;
- full customer names where identifiers suffice;
- signed URLs, credentials, access/refresh tokens, encryption keys;
- unrestricted payload dumps;
- invoice/customer documents.

Use safe diagnostic IDs, job/session pseudonymous IDs, error categories, versions,
and aggregate counters. A support bundle is previewable by the worker/admin before
submission.

## Threat model summary

### Threats

- stolen or shared device;
- malicious or over-privileged manager;
- cross-tenant identifier probing;
- leaked customer share link;
- server/operator access to raw evidence;
- model prompt injection through transcripts/documents;
- sync replay or idempotency collision;
- retention worker failure or clock rollback;
- logs/analytics leaking sensitive content;
- exported report containing unintended evidence;
- coercive workplace policy or deceptive consent.

### Required mitigations

- device encryption and app lock;
- MFA/trusted-device enrollment for sensitive business access;
- tenant/job/session authorization on every object operation;
- append-only audit and review events;
- short-lived, revocable, revision-scoped customer grants;
- content sanitization and source grounding for generated text;
- idempotent append semantics and replay protection;
- monotonic/conservative retention deadlines;
- privacy-safe logging and automated payload scans;
- immutable share previews;
- visible capture controls and worker-access guarantees.

## Privacy acceptance tests

1. A manager cannot remotely start a worker microphone.
2. A worker can start a permitted non-audio job session.
3. Raw audio does not appear in database event payloads, logs, crash reports, or
   analytics.
4. Deleting/redacting evidence invalidates search/index/cache access.
5. A customer link exposes only the selected immutable revision and evidence.
6. Cross-tenant IDs return no existence oracle.
7. A rejected candidate cannot affect payroll, billing, or customer reports.
8. A clock rollback does not extend a raw-media deadline.
9. A support role cannot fetch raw evidence without an audited break-glass action.
10. Model-training export is impossible without the separate consent state.
11. Worker export includes manager corrections and policy versions.
12. Ordinary telemetry can diagnose failure without raw content.

## Launch review checklist

Before each pilot jurisdiction or business model:

- recording and all-party consent requirements;
- employee versus independent-contractor rules;
- wage/hour, break, overtime, and record-access obligations;
- biometric/voiceprint restrictions;
- workplace notice and collective-bargaining considerations;
- customer-site confidentiality and safety policies;
- data residency and cross-border transfer;
- retention, legal hold, tax, and accounting requirements;
- incident response and breach notification;
- app-store background recording and permission rules.

Where requirements conflict with the product's worker-control principles, the
team should narrow or decline the deployment rather than hide the conflict.