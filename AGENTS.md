# Athlet-O application agent instructions

## Commerce, authorization, and data invariants

- Inventory may never become negative. Concurrent checkout for the final unit must serialize through database locks so exactly one order wins and the loser receives a clean insufficiency result. Keep lock ordering deterministic and test with independent database connections.
- Money movement is idempotent. Provider events and settlements deduplicate by the documented provider/reference keys, post ledger effects exactly once, validate amount and provider ownership, stamp paid state once, and never regress a paid order except through an explicit refund flow.
- Fulfillment changes are transactional. Ship only valid order states, preserve carrier/tracking and ETA semantics, and roll back completely for unknown, cancelled, unauthorized, or otherwise invalid orders—no leaked shipment or EDI state.
- Preserve cart holds, hold expiration, recurring-order scheduling, advisory-lock/double-fire prevention, order ownership, shipment ownership, and cross-user WebSocket isolation.
- Authorization and CSRF/host routing derive from verified configuration and identity. Never trust a requester-supplied `Host` value to establish the B2B surface; require the configured exact origin plus the `ALLOWED_HOSTS` gate and reject lookalike domains.
- Use integer minor units for money, explicit transaction boundaries for stock and payment state, verified user/tenant ownership, bounded errors, and secret-safe logging/telemetry.
- Database schema and migrations remain synchronized with SeaORM entities and application behavior. Do not hand-wave data migrations or weaken constraints to make tests pass.
- Run default unit/integration tests plus the documented ignored database-backed suites for order placement, concurrent oversell, payment idempotency/events/status, fulfillment, recurring orders/holds, ownership, and any affected money/stock workflow.

## Instruction discovery

Resolve `$PWD`, walk upward through every parent directory to the filesystem root, read every readable lowercase `agents.md` on that ancestor chain, and apply them root-to-leaf. Do not search sibling directories. Deduplicate resolved paths/inodes, avoid symlink cycles, and report unreadable files.

## Synchronize with the remote

Before editing, inspect `git status`, the current branch, configured remotes, and the default branch. Run `git fetch --all --prune` and create the feature branch from the latest remote default branch, not a stale local copy. Fetch again before pushing and incorporate upstream changes according to repository policy.

- avoid git rebase in favor of git merge.
- Never discard remote commits, force-push, rewrite shared history, bypass review, or bypass required CI.

## Resolve Git conflicts semantically

Resolve conflicts by understanding and combining both sides' intent. Do not mechanically choose `ours`, `theirs`, current, or incoming changes. Produce the conceptually correct result while preserving compatible inventory serialization, payment/ledger idempotency, fulfillment transactions, order and WebSocket isolation, recurring-order concurrency, host/CSRF authorization, schema constraints, tests, documentation, configuration, and public behavior. Rebuild generated entities, fixtures, or lockfiles from merged source rather than selecting one side's output. If intentions are incompatible, make the smallest explicit design decision and document it in the pull request.

After resolving:

1. Reread every affected file from the top, not only the conflict hunks.
2. Run formatting, clippy, default tests, all affected database-backed suites, migration checks, and relevant browser/E2E validation.
3. Search the entire worktree for unresolved conflict markers:

   ```sh
   grep -RInE '^(<<<<<<<|=======|>>>>>>>)' --exclude-dir=.git .
   ```

4. If any marker or suspicious partial resolution remains, repeat semantic resolution from the top and rerun validation.

A conflict is resolved only when the commerce and authorization behavior is conceptually coherent and verified, not merely when Git accepts the files.