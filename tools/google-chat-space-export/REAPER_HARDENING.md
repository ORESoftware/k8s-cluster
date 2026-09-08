# Google Chat reaper hardening

Tracking: `DEN-3473`, `DEN-3497`, and parent `DEN-266`.

## Problem fixed

The daily reconciliation workflow previously generated an empty
`coverage-evidence.json` placeholder. The receipt correctly failed closed, but
no actionable candidate could ever acquire Linear ownership or GitHub evidence,
so the scheduled process was structurally unable to make progress.

`reaper-materializer.mjs` replaces that placeholder with an idempotent,
privacy-preserving materialization stage.

## Pipeline

1. Fetch the fixed `alex-alex-me` Google Chat space through the protected Apps
   Script bridge.
2. Sanitize the private export before any Linear or GitHub operation.
3. Export a private Linear index and feed it to `import-plan.mjs` for duplicate
   prevention.
4. Materialize each candidate:
   - non-actionable candidates are excluded without external mutation;
   - privacy-bearing, credential-bearing, context-only, unsafe, or deceptive
     review candidates are excluded before any Linear or GitHub mutation;
   - other ambiguous candidates receive one durable review issue and are
     quarantined from automated completion;
   - actionable candidates reuse an exact issue or create one canonical child
     issue under `DEN-3473`;
   - existing issues receive only a content-free provenance section;
   - GitHub pull requests and default-branch commits are independently queried
     using the resulting Linear identifier.
5. Generate the existing content-free reconciliation receipt.
6. Upload only fetch totals, safety totals, content-free mutation totals, and the
   receipt. Raw pages, sanitized message bodies, the Linear index, import plan,
   and coverage evidence remain in the private runner directory.
7. Fail closed while any actionable candidate lacks real implementation
   evidence. Creating a planning ticket alone is not reported as completion.

## Idempotence and duplicate prevention

Every candidate carries a deterministic key of the form:

```text
google-chat:<space-id>:<24-hex-digest>
```

The materializer places that key in an HTML comment in the Linear issue. A
rerun searches for the marker before creating anything. The workflow-level
concurrency lock prevents overlapping live runs, and a per-run create circuit
breaker defaults to 25 issues.

Source message identifiers are persisted only as SHA-256 digests in Linear.
Message bodies, sender identities, contact values, credentials, and
secret-bearing URLs are never included.

## Review disposition gate

Manual-review candidates are classified before `ensureLinearIssue` runs. The
classification is deliberately narrow and deterministic:

- `privacy_sensitive`: the sanitized title proves that a secret, email address,
  or phone number was present;
- `context_only`: the item is only an agent/thread coordination instruction or
  a prompt preamble, not standalone product work;
- `unsafe_or_deceptive`: the item requests evasion or misrepresentation in the
  currently recognized high-risk prompt families;
- `requires_human_review`: the request is ambiguous but still plausibly valid
  engineering work, so exactly one quarantined Linear owner is permitted.

Excluded candidates retain their content-free candidate key and reason code in
the private evidence and public summary counts, but they do not create a Linear
issue and they do not trigger a GitHub evidence search. Existing false-positive
review shells should be canceled or linked as duplicates without deleting their
content-free provenance.

## Evidence semantics

- **Excluded**: non-actionable or rejected by the pre-mutation review disposition
  gate.
- **Quarantined**: ambiguous but plausibly valid engineering work; a human-review
  owner exists, but the item cannot be completed automatically.
- **Covered**: one canonical Linear owner plus an open/merged implementation PR
  or a verified default-branch commit.
- **Gap**: a valid engineering item has Linear ownership but no independently
  verified implementation evidence yet.

The final receipt remains the authority. A green fetch, a created issue, a
status claim in chat, or a bare SHA is not enough.

## Required protected configuration

The repository must provide these GitHub Actions secrets:

- `CHAT_BRIDGE_TOKEN`
- `LINEAR_API_KEY`

The workflow uses the ephemeral `github.token` with read-only permissions for
GitHub evidence lookup. No personal access token is stored by the workflow.

Optional environment controls:

- `LINEAR_TEAM_ID`
- `LINEAR_PARENT_ISSUE`
- `GITHUB_ALLOWED_OWNERS`
- `REAPER_MAX_LINEAR_CREATES`

## Operator runbook

1. Merge the hardening PR after the validation job passes.
2. Confirm both protected secrets exist; rotate any value that was pasted into
   chat, email, an issue, or a URL.
3. Run `Daily Google Chat reconciliation` manually once.
4. Inspect the uploaded `reaper-summary.json` and
   `reconciliation-receipt.json` artifacts.
5. Keep `DEN-3473` in progress until the exact-window receipt is complete.
6. For remaining implementation gaps, work from the canonical Linear owners and
   rerun the workflow; it will reuse them instead of creating duplicates.
7. Reconcile legacy review shells by linking exact duplicates to their canonical
   owner and canceling privacy-sensitive, context-only, unsafe, or deceptive
   false positives; never paste their source bodies into comments.

A live run is never inferred from configuration or tests. Only a current
workflow receipt is operational evidence.

## Module boundaries

The reaper is split into small, independently syntax-checkable modules for HTTP retry logic, Linear mutations, GitHub evidence lookup, privacy-safe provenance, and materialization policy. The CLI re-exports the tested public surface, so tests exercise the same code used by the scheduled workflow.
