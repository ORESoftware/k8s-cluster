# USA-ACC REST API backend agent instructions

## Repository workflow and safety

- History is append-only. Never use destructive filesystem or Git operations to discard work or rewrite shared history, including `rm` on repository files, `git rebase`, `git reset`, force pushes, history filters, `git clean`, destructive checkout/restore, branch/tag deletion, or amending pushed commits.
- Fix mistakes with a new commit or reviewed `git revert`. Changes land through small feature-branch pull requests.
- This repository is the source of truth. Any copy under `ORESoftware/k8s-cluster/remote/deployments` is a secondary checkout; merge here first and then bump the submodule pointer. Do not edit the secondary checkout directly.
- Preserve the dotted `.nix/` development-shell convention and keep `flake.nix`/`flake.lock` synchronized and reproducible.
- Path dependencies may resolve only inside the documented `k8s-cluster` superproject layout. Keep standalone CI honest about what it can validate; do not remove hygiene or formatting checks merely because full integration requires the superproject.
- Preserve API authentication, authorization, data validation, observability, migrations, and failure semantics when changing backend behavior.

## Instruction discovery

Resolve `$PWD`, walk upward through every parent directory to the filesystem root, read every readable lowercase `agents.md` on that ancestor chain, and apply them root-to-leaf. Do not search siblings. Deduplicate resolved paths/inodes, avoid symlink cycles, and report unreadable files.

## Synchronize with the remote

Before editing, inspect `git status`, current branch, configured remotes, and the default branch. Run `git fetch --all --prune` and create the feature branch from the latest remote default branch. Fetch again before pushing and merge upstream changes on a clean working tree.

- avoid git rebase in favor of git merge.
- Never discard remote commits, force-push, rewrite shared history, bypass review, or bypass required CI.

## Resolve Git conflicts semantically

Resolve conflicts by understanding and combining both sides' intent. Do not mechanically choose `ours`, `theirs`, current, or incoming changes. Produce the conceptually correct result while preserving source-of-truth ownership, append-only history, Nix reproducibility, superproject build assumptions, API/security behavior, migrations, tests, documentation, configuration, and runtime contracts. If intentions are incompatible, make the smallest explicit design decision and document it in the pull request.

After resolving, reread every affected file from the top, run formatting, linting, tests, builds available standalone, and full superproject validation when required, then search the entire worktree for conflict markers:

```sh
grep -RInE '^(<<<<<<<|=======|>>>>>>>)' --exclude-dir=.git .
```

If any marker or suspicious partial resolution remains, repeat semantic resolution from the top and rerun validation. A conflict is resolved only when the result is conceptually coherent and verified, not merely accepted by Git.