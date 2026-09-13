# DEN-2786 plaintext migration manifest

Linear: [DEN-2786](https://linear.app/denman/issue/DEN-2786) · Manifest gate: [DEN-2949](https://linear.app/denman/issue/DEN-2949)

GitHub: [ORESoftware/k8s-cluster#1104](https://github.com/ORESoftware/k8s-cluster/issues/1104) · Implementation: [ORESoftware/k8s-cluster#1123](https://github.com/ORESoftware/k8s-cluster/pull/1123)

## Purpose

`catalog/namespaces/migration-manifest.json` is the reviewable Phase 0 ledger for every legacy namespace occurrence in the exact-head inventory. It contains metadata and migration controls only. It does not contain secret values and it does not authorize provider writes.

The manifest is generated deterministically from:

- the exact-head namespace inventory rendered by `tools/namespace_migration.py inventory`;
- `catalog/namespaces/owners.json`;
- `catalog/namespaces/migration-rules.json`.

The source SHA-256 digests are recorded in manifest metadata. The manifest is canonical sorted JSON with a trailing newline, so independent jobs can reproduce it byte for byte.

## Occurrence identity

Every row is keyed by the exact tuple:

```text
(path, line, column, system, current)
```

The row ID is the SHA-256 of that canonical identity. Validation rejects duplicate IDs, duplicate identities, missing inventory identities, and manifest-only identities.

The initial corrected inventory contained 1,134 rows. That historical number is not a permanent invariant: repository changes legitimately change the exact-head inventory. The current contract instead requires the generated inventory and committed manifest to have identical occurrence identities and counts. The latest certified PR-head evidence before this update contained 1,251 entries with zero diagnostics and no encoded staging files.

## Safety state

The top-level `executionAuthorized` field is `false`. Every row also has `destructiveCleanupAllowed: false`.

Rows classified as `unclassified` are deliberately non-actionable:

- `owner: unclassified`;
- `target: null`;
- `reviewState: blocked`;
- `migrationMode: manual-review`;
- no invented consumers or grants.

A review-required row may carry a proposed target or template, but it remains non-executable. An unresolved target template never becomes a concrete target.

## Ownership and grants

All owners and consumers must be present in the checked-in owner registry. Every cross-owner consumer has one explicit read grant in `consumerGrants`; the manifest does not infer access merely because a repository or test job consumes a service.

A product, shared service, or test owner cannot target `ores/` without an explicit approved exception containing a reason, approver, and issue link. Slash targets otherwise remain under the registered owner root. Test owners remain separate non-production owners under the `*-test` boundary.

Validation also rejects distinct current resources collapsing onto one target. Repeated occurrences of the same current resource may share a target because they represent multiple references to one migration.

## Required controls per row

Every entry includes:

- environment and workload fields, nullable when they are not yet resolved or not applicable;
- a migration mode appropriate to the naming system;
- a review state;
- a non-empty verification procedure;
- a non-empty rollback procedure;
- consumers and explicit cross-owner grants;
- a nullable platform-target exception;
- a hard prohibition on destructive cleanup during Phase 0.

The procedures are system-specific. Secret paths use copy/verify/cutover; Kubernetes metadata uses dual emission and reader or selector rollover; host paths require state-aware backup and restoration; source packages require dependent updates; generated packages require generator-first regeneration.

## Commands

Generate the canonical file:

```bash
python3 tools/namespace_manifest.py generate --root .
```

Validate schema presence, source digests, deterministic output, exact coverage, ownership, grants, target roots, collisions, and staging-file removal:

```bash
python3 tools/namespace_manifest.py check --root . --format text
python3 tools/test_namespace_manifest.py
python3 tests/namespace_migration_workflow_test.py
```

Render without writing:

```bash
python3 tools/namespace_manifest.py render --root . > /tmp/migration-manifest.json
cmp /tmp/migration-manifest.json catalog/namespaces/migration-manifest.json
```

## CI certification boundary

The namespace migration workflow must certify pull-request heads and both `dev` and `main` push commits. `main` coverage is intentional: a final merge commit can combine two individually green histories into a tree that neither PR-head run tested.

The workflow checks out the selected exact source SHA, renders the exact-head inventory before deriving a manifest candidate, runs classifier and manifest adversarial tests, validates the committed canonical manifest, applies the PR new-reference ratchet, and uploads the exact-source inventory plus rendered manifest candidate as evidence.

Any change to inventory, registry, rules, generator, schema, manifest, or repository text that can affect namespace discovery must pass this exact-head workflow. An independent credential-free canary from a `*-test` organization remains the additional cross-repository acceptance boundary. Provider-backed tests remain a later gated phase and must use test-scoped identities rather than account-wide credentials.
