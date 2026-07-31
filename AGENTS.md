# Repository agent instructions

These instructions apply to this repository and work performed beneath it.

## Discover instructions hierarchically

Resolve `$PWD`, then walk upward through every parent directory to the filesystem root. Read every readable lowercase `agents.md` on that ancestor chain and apply them in root-to-leaf order. Do not search siblings. Deduplicate resolved paths/inodes, avoid symlink cycles, and report unreadable instruction files.

## Synchronize with the remote

Before editing, inspect `git status`, the current branch, remotes, and the default branch; run `git fetch --all --prune`; and cut the feature branch from the latest remote default branch rather than a stale local copy. Fetch again before pushing and incorporate upstream changes with `git merge` or `git pull` on a clean working tree.

- avoid git rebase in favor of git merge.
- Do not force-push, discard remote commits, rewrite shared history, bypass review, or bypass required CI unless explicitly authorized.

## Resolve conflicts semantically

Resolve Git conflicts by understanding and combining the intent of both sides. Never mechanically choose `ours`, `theirs`, current, or incoming changes. Produce the conceptually correct merged result, preserving compatible behavior, invariants, tests, documentation, configuration, and API contracts from both sides. When intentions are incompatible, make the smallest explicit design decision and document it in the pull request.

After resolving conflicts, review every affected file from the top, not only the conflict hunks. Run relevant formatters, linters, tests, and builds. Then search the entire worktree for unresolved markers, excluding `.git`:

```sh
grep -RInE '^(<<<<<<<|=======|>>>>>>>)' --exclude-dir=.git .
```

If any marker or suspicious partial resolution remains, repeat the semantic resolution process from the top and rerun validation. A conflict is resolved only when the merged result is conceptually coherent and verified, not merely when Git accepts the file.

## Change discipline

Keep changes scoped, preserve repository conventions, update tests when behavior changes, and record validation and residual risk in the pull request.

## Repository-specific rules

- **This repository is the source of truth.** The copy vendored into `ORESoftware/k8s-cluster` (under `remote/deployments/`) is a *secondary* submodule checkout — after merging here, bump the submodule pointer there. Do not edit the vendored copy directly.
- This repo is standalone (no superproject path deps): `cargo check` and `cargo test` run anywhere, and CI runs the full suite.
