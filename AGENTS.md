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
