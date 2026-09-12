# Cross-repository workflow dependency contract

Tracking: DEN-801 and DEN-1321.

GitHub Actions workflows that check out another repository are part of a distributed build graph. A repository-local green check is insufficient when the workflow silently follows an obsolete feature branch or an unreviewed mutable ref.

The versioned ledger at `policy/cross-repo-workflow-dependencies.v1.json` records the currently governed slice. Supported policies are:

- `immutable_commit`: the checkout is pinned to a lowercase 40-character commit through a literal or named workflow environment variable;
- `canonical_main`: the reviewed dependency intentionally follows `main`;
- `default_branch`: the checkout omits `ref` and follows the dependency repository's configured default branch;
- `feature_branch`: a temporary reviewed dependency with a Linear owner, PR number, and expiry.

The validator also scans workflow defaults and ref variables for `agent/*` values. Existing temporary refs require explicit, dated exceptions. New unapproved feature refs fail CI; expired exceptions fail; and exceptions fail once their workflow no longer needs them, forcing cleanup rather than permanent allowlisting.

This initial slice governs the coordinator GitOps pin, the cluster E2E repository checkout, and every feature-ref workflow discovered in the August 1, 2026 audit. DEN-1321 expands the ledger to all cross-repository checkout blocks and adds live PR-state reconciliation, reusable browser evidence machinery, and execution-only carrier metadata.

The report is emitted as JSON and Markdown in GitHub Actions. It contains no tokens, private repository content, or workflow inputs.
