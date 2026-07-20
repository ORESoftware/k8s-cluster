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

"Sync with the remote" (or just "sync") is **bidirectional and always contacts
the remote** — it fetches *and* pushes, never push-only. A clean local working
tree does **not** by itself mean "synced": a sync is not finished until local
and the remote have exchanged commits in both directions.

How to sync:

1. `git fetch --all --prune` — always safe; it only updates remote-tracking
   refs and never touches your working tree, so run it any time.
2. Make the working tree **clean before you pull/merge**: `git add` +
   `git commit` your work (or `git stash`). **Only `git pull` / `git merge`
   when the tree is not dirty** — pulling into a dirty tree makes git refuse
   the merge or tangle uncommitted edits with the incoming commits.
3. `git pull` (which fetches + merges) — or `git merge` the upstream tracking
   branch — to integrate the remote's commits into your now-clean branch.
4. `git push` — publish your commits so the remote has them too.

Integrate with **`git merge`** / **`git pull`** (which merges). **Never
`git rebase`** to sync — it rewrites history and breaks shared branches.
