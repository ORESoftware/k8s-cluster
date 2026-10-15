# Explicit-window, per-message audit evidence

Tracking: DEN-266 and ORESoftware/my-ai#119. Implementation is part of
ORESoftware/k8s-cluster#1541. This adds internal audit evidence utilities; it does
not replace either product contract authority, the existing 15-day scheduled
reconciliation, the sanitizer, or the issue/PR inventory collector.

## Separate four different claims

`message-coverage.mjs` distinguishes source completeness reported by an extractor,
message-by-message accounting, recorded issue/PR links, and actual implementation
verification. It **never authorizes an implementation-complete claim**. A URL,
review annotation, count, digest, or green test of this utility cannot prove a
product feature is implemented.

The input boundary is a trusted extractor's metadata descriptors, not raw Chat
bodies. Each descriptor contains exactly:

- `messageName`, `threadName`, `createTime`, nullable `lastUpdateTime`;
- the extractor's full `bodySha256`, `attachmentCount`, and boolean `deleted`.

Use full-body hashes, not shortened display hashes. The caller is responsible for
binding these descriptors to the exact verified export and for not logging input.
Google's listing API omits system messages; deleted tombstones have no message
content. A complete API listing is not a promise to recover those unavailable
contents. See the official reference:
https://developers.google.com/workspace/chat/api/reference/rest/v1/spaces.messages/list

## Build and reconcile

```js
import { buildMessageInventory, reconcileMessageCoverage }
  from './message-coverage.mjs';

const inventory = buildMessageInventory({
  records: trustedDescriptors,
  source: {
    kind: 'bridge-export', // or 'retained-ledger' for historical sanitized records
    artifactSha256: verifiedExportDigest,
    paginationComplete: verifiedPaginationTermination,
    snapshotTime: verifiedSnapshotTime,
  },
  windowStartInclusive: '2026-08-08T05:00:00Z',
  windowEndExclusive: exactAuditCutoff,
  provenanceKey: approvedPrivateAuditKey, // Uint8Array, 32–64 bytes, not a bridge token
});
const receipt = reconcileMessageCoverage({ inventory, review: reviewedAnnotations });
```

UTC instants retain nanosecond precision. The start is inclusive and the end is
exclusive. August 8 midnight in America/Lima is `05:00Z`; the earlier September 8
export used `04:00Z` (America/New_York). Preserve that earlier snapshot's stated
window rather than silently relabeling it. This API permits explicit windows
beginning at or after the fixed bridge floor; it does not widen bridge access.

The audit key creates domain-separated HMAC message/thread/version identifiers.
Do not commit the key. An approved private runtime store must retain it when
repeatable future joins are needed. Key rotation deliberately breaks those joins.
The output excludes raw message/thread names and original body hashes. Source
artifact digests and explicit GitHub references remain visible in the receipt.

## Review annotations

`review` has exactly `schemaVersion: 1`, the exact `inventoryDigest`, and `entries`.
Each entry binds `messageKey` and `versionDigest` and records:

- `disposition`: actionable, blocked, superseded, context, credential_only,
  private, reference, acknowledgement, or deleted;
- `requirements`: zero rows for nonactionable messages; one or more for actionable
  or blocked messages. Each row has an opaque uppercase `key`, `issues` containing
  canonical issue URLs, and `pullRequests` containing `{url, headSha, kind}`;
- `attachmentReview`: `{reviewedCount, evidenceDigest}`. Zero reviews require a null
  digest. Positive counts require a full digest of separately retained review
  evidence; metadata alone is not review evidence;
- optional `supersededBy`: another in-window message key. Chains must terminate at
  a reviewed actionable/blocked message; cycles and acknowledgement targets fail.

PR kind is `implementation` or `audit`. Audit-only PRs do not count toward
`requirementsWithImplementationPrLinks`. Every PR annotation needs an immutable
40-character head SHA. These links are candidates, not independently verified
issue ownership, test execution, review approval, or deployment evidence.

Distinct messages with identical text remain distinct. Exact duplicate page rows
are counted once and reported; differing versions of the same message stop the
import, even outside the selected window. Missing review rows remain visible.
Every attachment needs its own review, including on acknowledgements and
superseded messages. Deleted content remains explicitly unavailable.

## Output and limits

Receipts contain an inventory binding, deterministic review/receipt digests,
per-message dispositions, canonical requirement references, and separate counts.
`accountingComplete` requires a nonempty inventory with a disposition for every
record. `sourceCompletenessReported` is always false for a retained ledger,
incomplete pagination, or a snapshot earlier than the requested cutoff.
`attachmentReviewRecorded` describes annotations, not independent binary checks.

`sourceCompletenessIndependentlyVerified`, `issueCoverageVerified`,
`implementationCoverageVerified`, and `completionClaimAllowed` are always false.
Independent code, CI, review and deployment checks must run in the next stage.
No action in this module creates issues, merges PRs, retrieves credentials,
downloads attachments, or mutates Google Chat.

Inputs use closed field sets, bounded arrays and value-free errors. The current
limits are 100,000 descriptors/review entries, 100 attachments per message,
64 requirements per message and 32 references of each kind per requirement.
Caller-side input-byte limits remain necessary before parsing untrusted JSON.

Run the synthetic suite with:

```sh
node --test tools/google-chat-space-export/test/message-coverage.test.mjs
```

The exact-head workflow also runs the existing metadata-collector tests on Node
22 and 24. Neither job receives a bridge credential or uses real Chat contents.
