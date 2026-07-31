# ADR-0003: Local-first capture with selective encrypted synchronization

- **Status:** Accepted for incubation
- **Date:** 2026-07-31
- **Owners:** DEN-993, DEN-999, DEN-1001, DEN-1002

## Context

Contractors work in basements, mechanical rooms, rural sites, construction zones,
and customer properties with unreliable connectivity. A cloud-first design could
lose the start of a job, block notes, or create uncertainty about whether time was
recorded. At the same time, unrestricted upload of raw workplace audio creates
unnecessary privacy, security, storage, and trust risk.

The product needs local durability, offline review, multi-device collaboration,
selective evidence sharing, and organization-level reporting without treating the
cloud as the only source of newly captured work or uploading every raw artifact.

## Decision

The worker device is the first durable writer for job-session events.

- Start, pause, resume, stop, notes, evidence metadata, candidates, and review
  events are persisted locally before the UI acknowledges them.
- Core job capture and review remain functional offline.
- Client-generated IDs and stable idempotency identities allow later append.
- Synchronization exchanges append-only events and explicit projections/tombstone
  or availability transitions.
- Derived observations and approved business records synchronize by default under
  policy.
- Raw audio/photos/receipts synchronize only when selected or explicitly required
  by an understood job policy.
- All synchronized sensitive content is encrypted in transit and at rest; raw
  evidence uses separately scoped keys and authorization.
- Conflicting human reviews are preserved and resolved by a superseding event,
  not last-write-wins replacement.

## Local storage responsibilities

The device stores:

- cached job, assignment, rate, and capture-policy versions;
- job-session lifecycle;
- source observations and evidence metadata;
- candidate spans and inference provenance;
- human review events;
- local projections;
- upload/sync queue, per-device cursors, acknowledgements, and retry state;
- retention deadlines and redaction/deletion work;
- enough audit state to explain offline actions after reconnection.

Sensitive local storage is encrypted and tied to device/account recovery policy.

## Synchronization contract

Each envelope should include:

- contract/envelope version;
- tenant, job, session, and device scope;
- event ID and type;
- source producer/version;
- idempotency key;
- occurred/recorded timestamps, timezone, UTC offset, and monotonic offset where
  available;
- payload hash/signature or integrity metadata as appropriate;
- dependency/source IDs;
- local sequence/cursor metadata;
- encryption and content-class metadata.

Server behavior:

- accept or idempotently recognize the same semantic event;
- reject a different payload under the same idempotency identity;
- authorize every object independently of client claims;
- expose per-device acknowledgement/cursor state;
- preserve out-of-order arrivals without rewriting event history;
- rebuild deterministic projections when dependencies arrive;
- surface unresolved conflicts;
- avoid an existence oracle across tenants.

## Selective evidence sync

Evidence storage scope is explicit:

- `localOnly` — artifact remains on the originating device;
- `selectivelySynced` — worker/policy selected the artifact for encrypted upload;
- `tenantStorage` — artifact is governed by an explicit organization/job policy.

Derived observations can reference unavailable or expired evidence metadata. A
business record should remain explainable without requiring indefinite raw-media
retention.

## Failure behavior

### No network

Continue capture and review. Show queue age and last successful sync without
blocking the job.

### Server rejects authorization

Keep local data, stop retry storms, show the affected scope, and require account or
assignment repair. Do not delete the worker's local record.

### Idempotency collision

Quarantine the event, preserve both local versions for diagnosis, and require a
safe resolution. Do not choose one payload silently.

### Device clock changes

Use monotonic/session ordering where available and surface cross-device
uncertainty. Clock rollback must not extend retention deadlines.

### Device storage pressure

Protect acknowledged events and approved records first. Enforce raw-media
retention or pause raw capture visibly. Never silently discard acknowledged work
records.

### Evidence upload interrupted

Keep the encrypted local copy and resumable upload state. Do not expose a partial
object as available or shared.

### Conflicting review

Store both events, stop the affected projection from becoming final, and request a
superseding human resolution.

## Consequences

### Positive

- field capture survives cloud and connectivity outages;
- lower latency and clearer worker ownership;
- reduced raw-media exposure and storage cost;
- synchronization is replayable and auditable;
- model/report services can be unavailable without losing source work;
- conflicts are explicit.

### Negative

- more complex client storage, migration, and recovery logic;
- multi-device ordering and conflicts require careful UX;
- local encryption/key recovery becomes critical;
- retention/deletion must operate across local and cloud stores;
- support needs privacy-safe device diagnostics.

## Alternatives considered

### Cloud-first streaming

Rejected. It fails on real job-site connectivity and makes cloud availability a
precondition for recording work.

### Upload every raw session

Rejected. It is privacy-invasive, expensive, and unnecessary for most timeline,
report, and invoice workflows.

### Last-write-wins synchronization

Rejected for consequence-bearing records because it can erase worker/manager
review history and produce non-explainable accounting changes.

### Manual export only

Rejected as the complete architecture. It may be an MVP fallback, but does not
support crews, customer sharing, durable organization policy, or integrations.

## Fitness tests

1. Complete a job and review offline, reconnect, and receive one copy of every
   event.
2. Retry the same append batch and receive idempotent acknowledgements.
3. Reuse an idempotency identity with different content and receive a collision.
4. Create concurrent corrections on two devices and preserve both.
5. Upload a selected evidence clip without uploading adjacent raw segments.
6. Interrupt an upload and verify no partial shared artifact.
7. Expire/delete local evidence and propagate availability without deleting
   approved business lineage.
8. Roll the wall clock backward and verify retention does not extend.
9. Lose authorization while offline and preserve local access/export.
10. Diagnose sync failures through metadata without raw evidence.

## Review trigger

Revisit storage or transport mechanisms as platforms evolve, but retain the
architectural outcomes: local durable capture, offline usefulness, append/idempotent
sync, explicit conflict history, and selective—not indiscriminate—raw evidence
transfer.