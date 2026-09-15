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

1. Run the credential-free syntax, runtime-contract, fixture, and peer-authority
   gates. The protected live job depends on this job and cannot start if it
   fails.
2. Fetch the fixed `alex-alex-me` Google Chat space through the protected Apps
   Script bridge.
3. Sanitize the private export before any Linear or GitHub operation.
4. Export a private Linear index and feed it to `import-plan.mjs` for duplicate
   prevention.
5. Materialize each candidate:
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
6. Generate the content-free reconciliation receipt and validate it through the
   same executable runtime contract used by materialization.
7. Upload only fetch totals, safety totals, content-free mutation totals, and the
   receipt. Raw pages, sanitized message bodies, the Linear index, import plan,
   and coverage evidence remain in the private runner directory.
8. Fail closed while any actionable candidate lacks real implementation
   evidence. Creating a planning ticket alone is not reported as completion.

## Peer TypeSpec and JSON Schema authorities

The contract subtree contains two independently authored authorities:

- `contracts/main.tsp` is the TypeSpec source;
- `contracts/authored.schema.json` is the JSON Schema Draft 2020-12 source.

Neither file is generated from, ranked below, or silently overwritten by the
other. The daily workflow pins
`ORESoftware/typespec-json-schema-validator` to immutable commit
`2281843126ab644607b11cf8281d84f382d68dfc`. TJSV generates a comparison-only
Schema B with the official TypeSpec emitter, compares the declaration and
behavioral surfaces in both directions, and emits a deterministic report,
SARIF file, generated witness, and digest-bound Contract IR. Any unexplained
difference stops the workflow before protected credentials or live mutations
are used.

The authored authorities cover the candidate-action vocabulary, evidence and
receipt dispositions, materialization operations, reason-code vocabulary,
coverage evidence, materialization summaries, and final reconciliation
receipts. The independently maintained `contracts/instances/` corpus includes
positive and negative examples. It proves extra freeform fields, negative
counters, and incomplete receipt identities are rejected by both authorities.

`reaper-contracts.mjs` is the executable JavaScript boundary. Materialization
and receipt generation call it before returning or writing evidence. Its tests
compare every runtime enum to the independently authored JSON Schema enum, then
exercise conditional invariants that basic data-shape declarations do not
express, including content-free field restrictions, counter accounting,
operation accounting, and completion consistency. This is an incremental
JavaScript runtime bridge; generated clients or other language projections must
consume the parity-approved Contract IR and add their own native conformance
fixtures before claiming support.

Generated witnesses and reports are evidence only. They do not authorize source
replacement, publication, deployment, or a preferred-authority fallback.

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

1. Merge the hardening PR only after the runtime tests, TJSV parity action,
   exact-head namespace contract, secret scan, and required repository checks
   pass at the same commit.
2. Confirm both protected secrets exist. Do not copy credential values into
   Chat, Linear, GitHub comments, email, artifacts, or URLs.
3. Run `Daily Google Chat reconciliation` manually once.
4. Inspect the uploaded TJSV report/Contract IR plus `reaper-summary.json` and
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

The reaper is split into small, independently syntax-checkable modules for HTTP
retry logic, Linear mutations, GitHub evidence lookup, privacy-safe provenance,
contract enforcement, and materialization policy. The CLI re-exports the tested
public surface, so tests exercise the same code used by the scheduled workflow.
