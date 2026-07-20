# Agent Rules

Rules for AI agents (and humans) working in this repository.

## Forbidden — destructive operations

Never run, script, or suggest any of the following here:

- `rm` / `rm -rf` on tracked or untracked files (stage removals with `git rm` on a reviewed branch instead)
- `git rebase` (interactive or otherwise)
- `git reset` (any mode — `--soft`, `--mixed`, `--hard`)
- `git push --force` / `--force-with-lease` / `--force-if-includes`
- `git filter-repo`, `git filter-branch`, BFG, or any history-rewriting tool
- `git clean`
- `git checkout -- <path>` / `git restore` that discards uncommitted work
- deleting branches or tags (local `-D` or remote)
- amending commits that have been pushed

## Required workflow

- History is append-only. Fix mistakes with a new commit or `git revert` — never by rewriting.
- Changes land on `main` via feature branches; keep commits small and reviewable.
- **This repository is the source of truth.** The copy vendored into
  `ORESoftware/k8s-cluster` (under `remote/deployments/`) is a *secondary* submodule
  checkout — after merging here, bump the submodule pointer there. Do not edit the
  vendored copy directly.

## Build context

Path dependencies (`../../libs`, `../../submodules`) resolve only when this repo is
checked out at its `remote/deployments/` path inside the `k8s-cluster` superproject.
Full builds happen there; standalone CI is limited to hygiene and format checks by design.

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
