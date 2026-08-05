# Meshy image-to-3D provider integration

Tracking: Linear `DEN-2465` (provider foundation, done) · Linear `DEN-2506` (durable operationalization, in progress) · organization tracker `daedalus-fab/.github#9`

## Purpose

Meshy is an upstream geometry-generation provider for Daedalus. It can turn one image or one to four views of the same object into candidate 3D assets. Daedalus remains responsible for source ownership and provenance, durable artifact storage, scale and topology review, mesh repair, manufacturing-route selection, simulation, inspection planning, and machine-release authorization.

A provider task in `SUCCEEDED` state is **not** a Daedalus machine release. The provider client emits a draft candidate, and the durable worker completes with an immutable release boundary:

```json
{
  "provider_success": "candidate_geometry_only",
  "review_state": "needs_review",
  "release_state": "blocked",
  "machine_ready": false
}
```

## Components

| Path | Responsibility |
| --- | --- |
| `crates/meshy-client/src/lib.rs` | Typed provider API client, request validation, task models, redacted authentication, retry metadata, and Daedalus candidate envelopes. |
| `crates/meshy-client/src/cli.rs` | Automation-safe direct provider command surface. |
| `crates/meshy-client/src/bin/dd-meshy-adapter.rs` | Direct provider-adapter executable. |
| `crates/meshy-job/src/lib.rs` | Provider-neutral resumable stages, submission-intent safety, polling, bounded artifact fetching, SHA-256 verification, idempotent archive receipts, and review-blocked result contracts. |
| `src/bin/meshy-job-worker.rs` | `run-one` adapter over the canonical RDS, JetStream-outbox, and Fiducia-fenced job-control layer. |
| `examples/meshy/*.json` | Provider requests and durable job request examples that explicitly ask for GLB, STL, and 3MF. |
| `scripts/ci/check_meshy_integration.py` | Credential-free source and configuration contract checked in CI. |
| `.github/workflows/meshy-durable-worker.yml` | Independent formatting, compile, unit-test, and source-contract checks for the durable stage crate. |

The provider and stage crates are intentionally independently buildable. The complete fabrication worker still uses private path dependencies supplied by the `k8s-cluster` superproject, while both standalone crates can be compiled and tested in an ordinary GitHub Actions checkout.

## Authentication and configuration

### Provider client

`MESHY_API_KEY` is required. The client sends it as a sensitive `Authorization: Bearer ...` header and does not expose the value through `Debug` or structured errors.

`MESHY_API_BASE_URL` is optional and defaults to `https://api.meshy.ai`. Non-local plain HTTP endpoints are rejected. Localhost HTTP is accepted only to support deterministic mock-server tests.

### Durable worker

The `meshy-job-worker` additionally uses:

| Variable | Purpose |
| --- | --- |
| `FABRICATION_DATABASE_URL`, `RDS_DATABASE_URL`, or `DATABASE_URL` | Canonical job/checkpoint database. The worker checks for the canonical schema and never executes DDL. |
| `FIDUCIA_BASE_URL` | Fiducia lock/lease service used for coarse ownership and fencing tokens. |
| `FIDUCIA_AUTH_TOKEN` | Optional bearer credential when Fiducia requires authentication. |
| `MESHY_ARTIFACT_ALLOWED_HOSTS` | Required comma-separated DNS suffix allowlist for provider artifact downloads. |
| `MESHY_ARTIFACT_STAGING_DIR` | Bounded temporary download directory; default `/var/lib/daedalus/meshy-staging`. |
| `MESHY_ARTIFACT_TIMEOUT_SECS` | Total artifact request timeout; default 1800 seconds. |
| `DAEDALUS_ARTIFACT_ARCHIVE_DIR` | Durable directory/PVC archive root; default `/var/lib/daedalus/artifacts`. |
| `DAEDALUS_ARTIFACT_URI_PREFIX` | Stable URI prefix stored in archive receipts; default `daedalus-artifact://archive`. |
| `FABRICATION_JOB_LEASE_SECS` | RDS/Fiducia lease duration; default 120 seconds. |
| `FIDUCIA_WAIT_SECS` | Bounded lock-acquisition wait; default 5 seconds. |
| `MESHY_WORKER_POLL_SECS` | Provider polling interval; default 5 seconds. |
| `MESHY_WORKER_MAX_WAIT_SECS` | Maximum wait in one worker claim; default 1800 seconds. |

Production deployment should inject provider, database, NATS, and Fiducia credentials from the cluster secret manager. Never commit a credential, put it in a request body, forward it to a browser, include it in Linear or GitHub Project fields, or log the process environment.

## Direct provider commands

```text
dd-meshy-adapter capabilities
dd-meshy-adapter create-image <request.json|->
dd-meshy-adapter create-multi-image <request.json|->
dd-meshy-adapter get-image <task-id>
dd-meshy-adapter get-multi-image <task-id>
dd-meshy-adapter wait-image <task-id> [timeout-seconds]
dd-meshy-adapter wait-multi-image <task-id> [timeout-seconds]
dd-meshy-adapter list-image [page-number] [page-size]
dd-meshy-adapter list-multi-image [page-number] [page-size]
dd-meshy-adapter delete-image <task-id>
dd-meshy-adapter delete-multi-image <task-id>
```

Example:

```bash
export MESHY_API_KEY='...'

cargo run \
  --manifest-path crates/meshy-client/Cargo.toml \
  --bin dd-meshy-adapter \
  -- create-multi-image examples/meshy/multi-image-to-3d.json
```

The creation response is normalized to `dd.fabrication.external-generation-task.v1`. Retrieve or wait for the task to obtain `dd.fabrication.external-geometry-candidate.v1`, which includes provider URLs, expiry, credit usage, release blockers, and required evidence.

Direct adapter commands are useful for diagnostics and explicit reconciliation. Automated production work should use the durable job worker instead.

## Durable job request

Create a canonical fabrication job whose `kind` is:

```text
meshy_image_to_3d
```

Its `request_payload` follows `daedalus.meshy-job.v1`. Each source image carries an owned asset identifier, an HTTPS URL, and the lowercase SHA-256 of the source bytes already retained by Daedalus.

See `examples/meshy/durable-job-request.json`:

```json
{
  "schema_version": "daedalus.meshy-job.v1",
  "images": [
    {
      "asset_id": "asset-front",
      "url": "https://assets.example.invalid/owned/front.png",
      "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    }
  ],
  "target_formats": ["glb", "stl", "3mf"],
  "ai_model": "meshy-6",
  "archive_prefix": "meshy",
  "max_artifact_bytes": 536870912
}
```

Run a claimed job:

```bash
cargo run --bin meshy-job-worker -- \
  run-one \
  --tenant tenant-acme \
  --job-id c22e13a1-0fc8-430f-a78b-8cb73a687f9d \
  --owner meshy-worker-1
```

A JetStream consumer or dispatcher supplies the tenant/job identity. JetStream is a delivery and wakeup mechanism; RDS remains the canonical state, stage, checkpoint, attempt count, and result source of truth.

## Billable-create safety

Meshy task creation may consume credits, so an HTTP retry is not equivalent to an idempotent job retry.

The worker uses a durable submission-intent boundary:

1. `MeshyJobEngine::prepare` creates a `SubmissionCheckpoint::Prepared` plus an in-memory `SubmissionPermit`.
2. The worker commits the prepared checkpoint through `ClaimedJobLease::checkpoint` **before** calling Meshy.
3. Only the matching in-memory permit can execute that create call.
4. A successful provider response is immediately checkpointed with its task ID.
5. A transport, timeout, unreadable response, or retryable provider failure during create is treated as an ambiguous outcome and is not automatically resubmitted.
6. A restarted process that sees `Prepared` but has no task ID no longer has the permit. It returns `provider_submission_unreconciled` instead of issuing another billable request.

Manual reconciliation locates the provider task and submits a new durable request with:

```json
{
  "existing_provider_task": {
    "task_id": "provider-task-id",
    "mode": "image-to-3d"
  }
}
```

This adopts the known task and resumes polling/archival without another create call.

## Durable execution sequence

1. Retain the original JPG/PNG assets and calculate SHA-256 before job creation.
2. Insert the canonical RDS job with an idempotency key and the `daedalus.meshy-job.v1` payload.
3. Publish the job wakeup through the transactional outbox.
4. Acquire the Fiducia lease and claim the RDS row with the returned monotonically increasing fencing token.
5. Commit the Meshy submission intent before the billable create.
6. Poll with bounded intervals while renewing both Fiducia and RDS leases.
7. Persist provider progress, final state, expiry, errors, consumed credits, and only the explicitly requested output URLs.
8. Download one artifact at a time through the DNS-suffix allowlist with redirects disabled and a streamed byte cap.
9. Reject empty downloads, HTML/JSON provider error documents, and obvious format-signature mismatches.
10. Calculate SHA-256 while streaming, sync the staging file, copy to the durable archive, verify again, and atomically commit the final object.
11. Commit one checkpoint per archived GLB/STL/3MF output so recovery resumes at the next missing format.
12. Complete the RDS job with source hashes, archive URIs, output hashes, byte counts, media types, provider expiry/credits, and the immutable review/release block.

No PostgreSQL transaction spans provider generation, polling, download, hashing, or archive I/O.

## Artifact download and archive policy

The default fetcher is intentionally fail-closed:

- HTTPS and port 443 only;
- no URL credentials or fragments;
- no IP-literal hosts;
- exact host or subdomain of a configured DNS suffix only;
- no HTTP redirects;
- bounded connect and total timeouts;
- Content-Length preflight plus streamed byte enforcement;
- SHA-256 and byte count calculated over the downloaded bytes;
- basic GLB/STL/3MF/OBJ/FBX/USDZ signature checks;
- no archive receipt until copied bytes verify against the staging digest.

The first archive adapter targets a durable directory or Kubernetes PVC. Object keys are deterministic and do not expose raw tenant or provider task identifiers. Replaying the same artifact accepts identical bytes; different bytes at the same object key fail with `archive_object_conflict`.

A future S3/R2 adapter should implement the same `ArtifactArchive` trait and preserve deterministic keys, digest/size verification, immutable-object behavior, and stable archive receipts.

## Request policy

The provider and durable stage layers reject ambiguous or ignored combinations before a request reaches Meshy:

- one to four source images;
- a retained lowercase SHA-256 for every source image;
- unique source asset identifiers;
- GLB/STL/3MF and other explicitly supported output formats without duplicates;
- PBR only when textures are enabled;
- at most one texture prompt or texture image;
- 100–300,000 target polygons for standard remesh and 100–15,000 for `meshy-t2` Smart Topology;
- no remesh/topology settings that Smart Topology would silently ignore;
- `meshy-t1` and `meshy-t2` only for single-image tasks;
- explicit `3mf` request when Daedalus needs that package;
- `origin_at` and multi-view thumbnails only with automatic sizing;
- HTTPS for every non-local provider endpoint and every source image URL;
- bounded artifact sizes and safe relative archive prefixes.

Automatic size estimation is advisory. Daedalus should prefer a known physical measurement, calibration object, scan, or inspection result before manufacturing.

## Failure, retry, and recovery behavior

Provider polling and artifact failures may be retried from the latest committed RDS checkpoint. Provider create failures with an unknown outcome are terminal to automatic execution and require reconciliation; this is deliberate credit-spend protection.

The RDS job-control layer supplies:

- idempotent job creation;
- transaction-scoped advisory locks for short transitions;
- Fiducia fencing tokens to reject stale workers;
- bounded attempts and retry delays;
- JetStream outbox messages committed with state transitions;
- lease heartbeats and an expired-job reaper;
- checkpoint versions that prevent stale writes.

Task deletion is irreversible and removes provider-hosted models. Daedalus must complete durable ingestion and verify archive hashes before deleting a provider task.

## Verification

Credential-free checks:

```bash
cargo fmt --manifest-path crates/meshy-job/Cargo.toml -- --check
cargo check --manifest-path crates/meshy-job/Cargo.toml --all-targets
cargo test --manifest-path crates/meshy-job/Cargo.toml --all-targets
python3 scripts/ci/check_meshy_integration.py
```

The standalone durable-stage tests cover:

- process loss after a persisted submission intent without a task ID;
- ambiguous provider create outcomes and prohibition of automatic resubmission;
- provider task-ID binding and requested-output filtering;
- resumable GLB/STL/3MF archive checkpoints;
- source/output SHA-256 provenance;
- idempotent archive replay and conflicting-byte rejection;
- artifact-host SSRF controls;
- preservation of `needs_review`, `release_state = blocked`, and `machine_ready = false`.

These tests use mocks and temporary directories. They do not submit a live Meshy task or consume provider credits.

## Remaining DEN-2506 work

This durable slice does not complete the whole program. Remaining work includes:

1. Promote/verify the canonical job-control schema through `pg-defs` and the cluster migration path tracked by the durable-execution program.
2. Add the production JetStream consumer that invokes `meshy-job-worker run-one`, ACKing only after the RDS transition commits.
3. Add an S3/R2 immutable archive adapter and production bucket/IAM lifecycle policy; the current adapter targets a durable directory/PVC.
4. Add webhook notification intake and SSE progress, both re-fetching authoritative provider/RDS state.
5. Create review-blocked candidate `fab_designs` records from verified archive receipts.
6. Dispatch STL/3MF through deterministic normalization, scale evidence, manifold/watertightness, normals, wall-thickness, orientation, and additive preflight.
7. Add provider budgets, tenant quotas, metrics, alerts, and dead-letter/reconciliation operations.
8. Add generated clients, paired Flutter/Rust desktop review flows, provider-neutral MCP tools, and cross-repository e2e/recovery drills.
9. Run one explicitly approved, credit-capped live smoke task after production credentials, durable storage, and release-blocking evidence are provisioned.
