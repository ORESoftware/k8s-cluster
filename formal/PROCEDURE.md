# Formal-methods change procedure

This backend issues upload sessions, records verified segment completion, mirrors encrypted objects, copies them to user-owned clouds, and deletes expired data. Those workflows combine durable metadata, object storage, retries, leases, and account boundaries. This procedure governs their formalization; it does **not** claim the planned models are already proved.

The checked machine inventory is [`procedure.toml`](procedure.toml).

## Change procedure

1. Identify the affected machine before changing upload status, completion, retention, mirror metadata, claim leases, retry/backoff, OAuth, or deletion behavior.
2. Model Postgres state, primary object state, mirror/cloud object state, worker claim, logical time, and HTTP delivery as separate facts. A successful object PUT is not automatically a verified completed segment.
3. State safety properties before choosing a checker. State liveness only with assumptions about advancing time, recurring drains, credentials, storage availability, and worker scheduling.
4. Use finite Quint/TLC or Apalache models for workflow schedules. Generate ITF traces and replay them against deterministic Rust transition seams with injected time, IDs, storage outcomes, and crash points.
5. Keep pull-request bounds small and reproducible; run wider retry, worker, and crash schedules periodically.
6. Report exact bounds, assumptions, model hash, implementation revision, tool versions, result class, and resource limits.

## Claim language

Use only: **typechecked specification**, **randomized exploration**, **bounded exhaustive verification**, **implementation replay**, **differential replay**, or **unbounded proof**. A bounded result must never be summarized as “Sonus storage is proved correct.” It must say whether S3/R2 semantics, PostgreSQL isolation, OAuth providers, email delivery, and real wall-clock scheduling were modeled or assumed.

## Counterexamples

Preserve the original trace and provenance, minimize it without deleting the failure, classify model defect versus implementation defect versus assumption mismatch, and add a deterministic Rust regression when production behavior diverges. Retain minimized traces under `formal/regressions/` once models land. Do not hide a failure by weakening an invariant without written rationale or by increasing a timeout alone.

## Required review triggers

Formal review is mandatory for changes to upload-session identity or status, presign/completion ordering, storage-key verification, retention eligibility, deletion claims, mirror claims and fingerprints, cloud-copy retries, transfer-pause leases, account deletion, or any metadata key used to recover those workflows.

## Initial modeling order

1. **Upload session.** Presign, encrypted PUT, completion, duplicate completion, stale session, and committed response loss.
2. **Retention deletion.** Eligibility, durable delete claim, primary/mirror deletion, crash/reclaim, and final metadata transition.
3. **Storage mirror.** Claim, copy, fingerprint binding, retry exhaustion, stale worker, and deletion interaction.
4. **Cloud copy.** OAuth credential availability, transfer pause, idempotent destination identity, bounded retry, and account revocation.

Canonical observations should contain segment status, session/generation identity, verified storage key, primary and mirror presence, claim owner/expiry, attempt count, next-attempt time, retention deadline, deletion stage, cloud destination state, and account identity—never tokens, presigned URLs, encryption keys, or raw audio.
