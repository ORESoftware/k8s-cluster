# Elenkos recovery quarantine

The DEN-3786 provenance review identified a dormant authorization error: matching
paths, modes, a commit message, and a fingerprint recomputed from the observed
tree do not establish that a predecessor was independently approved. No future
repository or tag mutation is authorized by the earlier publication receipts.

## This change

The legacy one-shot publisher is retired. It has no push trigger, no checkout,
no credential handoff, and no token permissions. A stale manual dispatcher gets
an explicit failure, never a successful-looking skipped publication.

The existing managed-tree patcher now transforms the recovery module into a
read-only verifier. It accepts only the exact expected tree and commit identity,
an already-matching initial tag, and stable head/tag rereads. Marker-only trees,
changed content, changed modes, missing tags, moved tags, and reference races are
refused before any mutation. Direct commit/tag mutator entry points are also
quarantined. Previously widened or partially quarantined source is refused.
Source is parsed and compiled before a file can be changed; it is never imported
or executed by the patcher.

This is containment, **not** the missing predecessor-approval implementation.
It does not lift the publication hold, certify current product deployments,
recreate inaccessible repositories, or move an existing release tag. Normal
application development after v0.1.0 must not be mistaken for bootstrap drift.

## Validation

```sh
python3 scripts/ops/test_elenkos_recovery_quarantine_20260829.py -v
```

Tests are hermetic and use only Python's existing standard library. No dependency
installation, registry access, GitHub credentials, or production API is required.
The dedicated PR workflow also checks transformation of the actual recovery
source, without executing the recovery module or any publisher.

## Before any successor can write

A separate reviewed design must bind approval to repository ID, an externally
approved predecessor commit/tree, the exact proposed successor, immutable
artifact provenance, explicit action permissions, expiry, and observed reference
preconditions. Approval must not come from the tree being approved. Add negative
conformance tests and an independently observed test-organization canary first.
Existing release tags must remain immutable. Re-enabling publication or deploying
a successor requires explicit approval; merging this containment patch does not
supply it.
