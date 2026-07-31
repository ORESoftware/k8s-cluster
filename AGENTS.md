# Repository agent instructions

Apply these instructions to this repository and all work beneath it.

## Instruction discovery

Resolve `$PWD`, walk upward through every parent to the filesystem root, read every readable lowercase `agents.md` on that chain, and apply them root-to-leaf. Do not search siblings. Deduplicate resolved files, avoid symlink cycles, and report unreadable files.

## Remote synchronization

Before editing, inspect `git status`, the current branch, remotes, and the default branch. Run `git fetch --all --prune` and create the feature branch from the latest remote default branch, not a stale local copy. Fetch again before pushing and incorporate upstream changes with `git merge` or `git pull` on a clean working tree.

- avoid git rebase in favor of git merge.
- Never discard remote commits, rewrite shared history, force-push, bypass review, or bypass required CI unless explicitly authorized.

## Semantic conflict resolution

Resolve Git conflicts by understanding and combining both sides' intent. Do not mechanically choose `ours`, `theirs`, current, or incoming. Produce the conceptually correct merge while preserving compatible behavior, invariants, tests, documentation, configuration, and API contracts. If intentions conflict, make the smallest explicit design decision and document it in the PR.

After resolving, reread every affected file from the top, run relevant formatters, linters, tests, and builds, then search the whole worktree for conflict markers:

```sh
grep -RInE '^(<<<<<<<|=======|>>>>>>>)' --exclude-dir=.git .
```

If any marker or suspicious partial resolution remains, repeat the semantic resolution process from the top and rerun validation. A conflict is resolved only when the result is conceptually coherent and verified, not merely accepted by Git.

Keep changes scoped, preserve repository conventions, update tests when behavior changes, and record validation and residual risk in the PR.
## Repository-specific rules

- **This repository is the source of truth.** The copy vendored into
  `ORESoftware/k8s-cluster` (under `remote/deployments/`) is a *secondary* submodule
  checkout — after merging here, bump the submodule pointer there. Do not edit the
  vendored copy directly.
- This repo is standalone — no path dependencies on the `k8s-cluster` superproject —
  so CI runs the full `cargo check` + `cargo test` suite (plus hygiene and an
  informational format check). Keep it that way: new dependencies come from
  crates.io, not `../../libs`.
