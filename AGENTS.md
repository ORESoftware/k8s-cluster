# Branch and Worktree Policy

## Repository role

- This is the static Astro marketing site only.
- Customer features belong to `fiducia-customer.rs`; operator features belong to
  `fiducia-admin.rs`.
- Do not add login, customer account, API-key, or admin workflows here.

- Work directly on the `main` branch for now.
- Before making changes, confirm that `main` is the checked-out branch.
- Do not create or use feature branches.
- Do not create or use Git worktrees.
- Merge any existing non-`main` branch into `main` with an intent-preserving merge, resolve conflicts semantically, and continue work on `main`.
- Push completed work to `origin/main`.
- Preserve existing uncommitted work and stop for operator guidance if moving to
  `main` cannot be done safely.

## Syncing with the remote

"Sync with the remote" (or just "sync") is a **two-way** exchange — pull the
remote's commits down **and** push yours up. It is never push-only, and a clean
local tree does not by itself mean "synced": you are done only once local and
the remote hold the same commits.

To sync:

1. **Commit your work first** (`git add` + `git commit`) so the tree is clean —
   pull/merge only into a clean tree. `git pull` / `git merge` aborts when an
   incoming change touches a file you have edited, and even when it doesn't it
   buries the merge in your uncommitted work. (Can't commit yet? `git stash`,
   then `git stash pop` after step 3.)
2. `git fetch --all --prune` — safe any time; it only updates tracking refs.
3. `git pull` (fetch + merge) — or `git merge` the upstream branch — to
   integrate the remote's commits.
4. `git push` to publish yours.

Integrate with **`git merge` / `git pull`**. **Never `git rebase` to sync** — it
rewrites history and breaks shared branches.
