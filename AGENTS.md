# Agent guidelines — fiducia-monorepo

## Branch and Worktree Policy

- Work directly on the `main` branch for now.
- Before making changes, confirm that `main` is the checked-out branch.
- Do not create or use feature branches.
- Do not create or use Git worktrees.
- Merge any existing non-`main` branch into `main` with an intent-preserving merge, resolve conflicts semantically, and continue work on `main`.
- Push completed work to `origin/main`.
- Preserve existing uncommitted work and stop for operator guidance if moving to
  `main` cannot be done safely.

## Syncing with the remote

"Sync with the remote" (or just "sync") is **bidirectional and always contacts
the remote** — it pulls *and* pushes. It is never push-only, and a clean local
working tree does **not** by itself mean "synced": a sync is not finished until
local and the remote have exchanged commits in both directions.

The steps for a sync:

1. `git fetch --all --prune` — see what the remote has.
2. `git pull` (which merges) — or `git merge` the upstream tracking branch —
   to integrate the remote's commits into your local branch **first**.
3. `git add` / `git commit` any local work.
4. `git push` — publish your commits.

Always integrate with **`git merge`** (and plain `git pull`, which merges).
**Do not `git rebase`** to sync — rebasing rewrites history and breaks shared
branches; keep the merge history instead.
