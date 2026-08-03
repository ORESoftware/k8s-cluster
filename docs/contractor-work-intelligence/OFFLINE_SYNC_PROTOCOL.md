# Offline sync protocol

**Status:** Incubation contract implemented; production client, service, and
operations are planned.

This document defines how the contractor work-intelligence sister app transports
append-only job records from an offline device to a tenant service. The
normative machine-readable artifacts live in the pinned interfaces repository:

- [`sync-batch.schema.json`](../../apps/sonus-auris-interfaces/incubation/contractor-work-intelligence/v1/sync-batch.schema.json)
- [`sync-semantics.mjs`](../../apps/sonus-auris-interfaces/incubation/contractor-work-intelligence/v1/sync-semantics.mjs)
- [`offline-review-sync-batch.json`](../../apps/sonus-auris-interfaces/incubation/contractor-work-intelligence/v1/fixtures/offline-review-sync-batch.json)

The protocol is part of the DEN-992 portable contract program and DEN-1542 sync
slice. It does **not** mean a production mobile sync worker, server endpoint,
queue, acknowledgement API, or cloud deployment already exists.

## Purpose

A contractor must be able to complete a job session, capture evidence, review a
timeline, and approve time without reliable connectivity. Later synchronization
must preserve what each device recorded and what each person approved. It must
not silently reorder authority, overwrite another device, duplicate billable
records, or turn a partial upload into a false success.

The protocol therefore transports immutable record versions with explicit
lineage. It is not a mutable row-replication format.

## Non-negotiable invariants

1. **Local persistence precedes acknowledgement.** A client records an envelope
   durably before telling the user that the action was saved.
2. **Array order is never authority.** Causal relationships and corrections use
   explicit IDs.
3. **Stable identity is immutable.** Reusing an envelope ID, device sequence, or
   idempotency identity for different content is a conflict.
4. **Duplicate delivery is safe.** Replaying the same immutable envelope does
   not create another domain record.
5. **Conflicts are quarantined.** The receiver never resolves an identity
   conflict with last-write-wins.
6. **Evidence is not accounting authority.** Sync delivery cannot bypass worker
   review or create approved time, reports, invoices, payroll, or discipline.
7. **Tenant and job scope are explicit.** Every envelope and attachment repeats
   tenant, job, and job-session scope.
8. **Evidence bytes are separate and encrypted.** Metadata never embeds raw
   audio, photos, credentials, signed URLs, or secret keys.
9. **Business history is append-only.** Corrections supersede prior versions;
   generic destructive deletion is absent.
10. **Uncertainty remains visible.** A rejected or conflicted envelope is not
    represented as synchronized or accepted.

## Contract layers

The sync package has three distinct layers.

### Structural schema

`sync-batch.schema.json` defines valid field shapes, enumerations, UUIDs,
timestamps, URI fields, hashes, and JavaScript-safe integer limits. Structural
validation is necessary but cannot establish cross-record meaning.

### Semantic validation

`sync-semantics.mjs` validates relationships that JSON Schema cannot express:

- canonical payload hashes;
- tenant/job/session consistency;
- dependency closure and cycle freedom;
- monotonic Lamport progress across local dependencies;
- linear, record-preserving supersession;
- stable device sequence and idempotency identities;
- evidence-lifecycle operation restrictions;
- encrypted attachment and path safety;
- replay classification.

A receiver must apply both structural and semantic validation before accepting a
batch.

### Domain validation

The `payload` remains a canonical domain record and declares its source schema
with `schemaUri`. The receiver must validate that payload against the referenced,
supported domain schema and then apply the work-ledger semantic validator. A
valid transport envelope does not make an invalid invoice, report, review, or
time entry valid.

## Sync batch

A batch groups one bounded upload attempt for one tenant. It includes:

- `syncContractVersion`;
- stable `batchId`;
- `tenantId`;
- batch creation timestamp;
- producer name, version, and environment;
- `knownEnvelopeIds` already confirmed by the receiver;
- append-only `entries`;
- encrypted `attachments` metadata.

`knownEnvelopeIds` permits an envelope to depend on a previously accepted remote
record without retransmitting that record. A client must populate it from an
authenticated acknowledgement or reconciliation response, not from an
unverified local guess.

Batches may contain entries in any order. Implementations may sort for transport
or display, but correctness cannot depend on that sorting.

## Record envelope

Each envelope carries one immutable version of a domain record:

```text
envelope identity
+ tenant/job/session scope
+ record type and stable record ID
+ operation
+ source schema URI
+ canonical payload hash
+ canonical payload
+ device sequence and Lamport clock
+ wall-clock and optional monotonic time
+ causal dependencies
+ optional supersession predecessor
+ attachment references
```

The envelope ID identifies a delivery artifact. The record ID identifies the
business record whose history the envelope participates in. Multiple envelopes
may refer to the same record only through one explicit, linear supersession
chain.

### Origin identity

`origin` contains:

- `deviceId`;
- positive `sequence` unique within tenant and device;
- positive `lamportClock`;
- `occurredAt` and `recordedAt`;
- optional `monotonicOffsetMillis`;
- stable per-device `idempotencyKey`.

Wall-clock timestamps support human interpretation and audit. They do not alone
determine causal order because device clocks can move. The Lamport clock and
explicit dependency graph carry transport causality. Monotonic offsets help
reconstruct one process lifetime but are not comparable across devices or
reboots.

## Canonical payload identity

The payload hash is SHA-256 over deterministic canonical JSON:

- object keys are sorted recursively;
- array order remains significant;
- unsupported values such as `undefined`, non-finite numbers, and unsafe integer
  values are rejected;
- UTF-8 JSON bytes are hashed;
- the resulting digest is lowercase hexadecimal.

Canonical hashing detects accidental or malicious content drift when the same
stable delivery identity is replayed. It is an integrity identifier, not a
replacement for authenticated transport, authorization, or attachment
cryptography.

## Causal dependencies

`dependsOnEnvelopeIds` states which prior envelopes must exist before an entry
can be applied. A dependency may refer to:

- another entry in the same batch; or
- an ID in `knownEnvelopeIds` representing confirmed remote state.

A local dependency must have a lower Lamport clock than the dependent entry.
Self-dependencies, unknown dependencies, and dependency cycles are invalid.

Examples include:

- an observation depending on the evidence record it references;
- a candidate span depending on source observations;
- a review depending on its candidate;
- approved time depending on the terminal worker review.

The generic transport validator proves graph integrity. Domain validators prove
that the declared dependencies are the right kinds of records for the payload.

## Supersession

A correction uses `supersedesEnvelopeId`. The predecessor must also appear in
`dependsOnEnvelopeIds`. Within a batch, a successor must preserve:

- tenant;
- job;
- job session;
- record type;
- stable record ID.

Supersession must be linear and acyclic. One predecessor cannot have two
successors, and one logical record cannot have independent roots. A receiver
must perform the equivalent checks against existing remote state inside the
same transaction or serialized acceptance boundary.

Supersession does not erase the prior envelope. It changes which record version
is terminal for projections that are authorized to use that history.

## Replay classification

Before applying an incoming envelope, the receiver compares it with records
having the same stable identity.

### New

No matching envelope ID, device sequence, or device idempotency identity exists.
The receiver may continue validation and application.

### Duplicate

A matching stable identity exists and immutable content agrees: scope, record
identity, operation, schema, payload hash, and supersession predecessor are the
same. The receiver returns the prior acknowledgement without creating another
record or attachment.

### Conflict

A matching stable identity exists but immutable content differs. The receiver:

1. rejects or quarantines the envelope;
2. records a content-minimized conflict event;
3. leaves prior accepted state unchanged;
4. exposes the conflict to an authorized user or support workflow;
5. never chooses the newest timestamp as an automatic winner.

A conflict is different from a valid later correction. Corrections use a new
envelope identity and explicit supersession.

## Evidence lifecycle operations

The contract supports only these non-append operations:

- `redactEvidence`;
- `expireEvidence`;
- `cryptographicallyEraseEvidence`.

They apply only to an `evidenceItem`, require a predecessor, and carry the
corresponding terminal availability state. They cannot target observations,
reviews, approved time, reports, or invoices.

This restriction prevents a generic transport delete from erasing accounting
or approval history. A future jurisdiction-specific deletion workflow must
preserve a minimal, non-sensitive audit marker where legally and contractually
permitted, while ensuring inaccessible evidence bytes and keys are actually
removed.

## Encrypted attachments

Raw evidence bytes are carried separately from batch metadata and remain encrypted in transit and at rest.

Attachment bytes are uploaded separately from batch JSON. The manifest contains:

- stable attachment and evidence-item IDs;
- repeated tenant/job/session scope;
- availability state;
- normalized relative path;
- ciphertext SHA-256;
- ciphertext byte length;
- media type;
- encryption algorithm and key version.

For an `available` attachment, path, hash, positive byte length, and encryption
metadata are mandatory. For `expired`, `redacted`, or
`cryptographicallyErased`, the path, hash, encryption metadata, and byte count
must be absent or zero.

Paths must be relative, normalized, traversal-free, and credential-free. A
manifest never carries an access token, signed query string, storage secret, or
private key. The server chooses any temporary upload URL through a separate,
least-privilege authorization flow.

The incubation contract conservatively requires the corresponding evidence-item
append to be present in the same batch. A future acknowledgement protocol may
allow attachment retries against already accepted evidence records, but must
retain identical scope and content identity checks.

## Receiver algorithm

A conforming receiver should process a batch in this order:

1. Authenticate the device/session and authorize the tenant.
2. Enforce batch-size, entry-count, attachment-count, and request-size limits.
3. Validate the structural schema.
4. Validate sync semantics and canonical payload hashes.
5. Load referenced known envelopes under the same tenant.
6. Re-check dependency and supersession constraints against remote state.
7. Validate each payload against its declared supported domain schema.
8. Apply domain semantic validation.
9. Classify every entry as new, duplicate, or conflict.
10. Reserve new device sequences and idempotency identities atomically.
11. Persist accepted envelopes append-only.
12. Issue least-privilege attachment upload work for accepted evidence.
13. Return per-envelope results; never reduce partial failure to a batch-level
    success boolean.

If the receiver commits records but its response is lost, the client retries the
same immutable envelopes. They classify as duplicates and receive the same
logical acknowledgement.

## Planned acknowledgement contract

DEN-1542 currently implements the upload-side schema and semantic rules. A later
revision must define a server acknowledgement with, at minimum:

- batch ID and server receipt ID;
- per-envelope status: accepted, duplicate, conflict, rejected, or deferred;
- canonical accepted envelope ID and payload hash;
- missing dependency IDs;
- attachment upload state;
- retryability and bounded backoff hints;
- content-minimized error codes;
- server cursor or checkpoint for reconciliation.

“Accepted” must mean durable domain-envelope persistence, not merely receipt by
an edge proxy or queue. Attachment completion is a separate state unless the
endpoint explicitly provides an atomic contract.

## Client behavior

The user interface must distinguish:

- saved locally;
- queued;
- uploading metadata;
- waiting for attachment transfer;
- synchronized;
- partially synchronized;
- conflict requiring review;
- blocked by authentication, policy, storage, or unsupported schema.

Capture continues offline within device safety limits. A sync error must not
silently stop local recording, rewrite a reviewed timeline, or mark a report as
delivered. Users need a bounded queue summary and a way to retry after restoring
credentials or connectivity.

## Security and privacy

- Authenticate every request and authorize tenant, device, job, and operation.
- Encrypt transport in addition to attachment-level encryption.
- Store connector or upload credentials outside the envelope.
- Rate-limit by tenant and device without logging precise location, transcripts,
  raw payload bodies, or attachment paths.
- Reject unsupported schema URIs rather than fetching arbitrary remote schemas.
- Treat hashes as identifiers that can still be sensitive when correlated with
  a tenant or job.
- Partition quarantine data by tenant and apply shorter retention where
  possible.
- Make redaction and cryptographic-erasure results observable and auditable
  without retaining erased content.

## Observability

Safe telemetry may include:

- batch and envelope counts;
- accepted, duplicate, conflict, rejected, and deferred counts;
- age of the oldest queued envelope;
- retry attempts and backoff class;
- attachment byte totals by broad media class;
- validation error codes;
- supported schema versions;
- duration percentiles.

Telemetry must not include raw payloads, transcripts, precise coordinates,
customer names, attachment paths, signed URLs, or credentials.

## Conformance tests

The interfaces repository tests at least:

- a valid, deliberately reordered offline job/review batch;
- structural and semantic validation;
- stable canonical hashing;
- new/duplicate/conflict replay classification;
- duplicate envelope, sequence, and idempotency rejection;
- unknown versus confirmed remote dependencies;
- tenant/job/session isolation;
- supersession branch, cycle, and cross-record rejection;
- evidence-lifecycle restrictions;
- attachment encryption, path, scope, and availability invariants;
- forbidden raw audio and credential keys;
- JavaScript safe-integer bounds.

Production clients and services must add cross-runtime golden-vector tests in
Rust, Dart, and TypeScript, database concurrency tests, process-death recovery,
network timeout after commit, attachment partial-failure, and multi-device
conflict tests.

## Relationship to opto-sync

An opto-sync adapter may implement persistence, queueing, retry, and transport.
That adapter must consume this public contract rather than exposing opto-sync
internal tables, clocks, conflict rows, or storage locations as sister-product
APIs. The sister app must also remain testable with a simple reference adapter,
so opto-sync is an implementation option rather than a hidden product boundary.

## Release gates

A production sync implementation is not releasable until:

- cross-runtime canonical hashes match golden vectors;
- repeated delivery cannot duplicate approved time or invoice basis;
- database concurrency preserves sequence and supersession uniqueness;
- conflict and partial-failure states are visible to users;
- attachment uploads use least-privilege, expiring authorization;
- redaction, expiry, and cryptographic erasure are verified end to end;
- tenant-isolation and authorization tests pass;
- offline endurance, process death, reboot, clock change, and delayed-sync tests
  pass on physical devices;
- telemetry redaction is independently reviewed;
- supported schema upgrade and replay behavior is documented.
