# Agent guidelines — sonus-auris-monorepo

Git superproject for the Sonus Auris repositories — each app is a git submodule under `apps/`, plus a Nix dev shell (`nix develop`).

## Instruction discovery

- Resolve the real path of `$PWD`, walk its ancestors through the filesystem root, and load every readable lowercase `agents.md` in root-to-leaf order.
- Do not search sibling directories. Deduplicate canonical paths, detect symlink cycles, and report unreadable instruction files.
- `AGENTS.md`, `.claude/CLAUDE.md`, `.gemini/GEMINI.md`, and `.openai/AGENTS.md` are compatibility pointers only; lowercase `agents.md` files are canonical.

## Linear mapping

- GitHub organization: `github.com/sonus-auris`.
- Linear project: `github.com/sonus-auris` in the Denman workspace.
- Locate or create the matching Linear issue before substantial work, and record PR links, tests, blockers, and remaining work there.

## Submodules

Every app lives under `apps/<name>` as a git submodule pinned to a commit on its `main`. Never `rm -rf` a submodule directory. To repoint a submodule, commit and push **inside** that submodule, then stage the updated gitlink here with `git add apps/<name>`. Put scratch checkouts / worktrees under `tmp/` (gitignored).

Never advance a gitlink merely because a child branch exists. Verify the exact child commit is on the child repository's `main`, confirm its required checks passed, then update and test the superproject pin through a focused PR.

## Command safety — STRICT (all agents MUST follow)

Never run destructive or irreversible shell commands. To remove or move files, **always go through git** so the change is tracked and recoverable.

**Blacklisted — do NOT run:**
- `rm`, `rm -rf`, `rmdir`, `unlink` — never delete via raw `rm`.
- bulk / indirect deletion: `find … -delete`, `find … -exec rm …`, `xargs rm` — no bypasses of the `rm` ban.
- raw `mv` of tracked files; truncating a tracked file with `>` or `truncate`.
- `git reset --hard`, `git clean -fdx`, `git checkout -- .` / `git restore .` mass-discard.
- `git stash drop` / `git stash clear`, `git branch -D`, `git tag -d` — destroy unmerged work / refs; not on shared branches unless the operator explicitly asks.
- `git push --force` / history rewrites on shared branches (especially `main`).
- `dd`, `mkfs`, `shred`, recursive `chmod -R` / `chown -R` on broad paths, fork bombs.

**Whitelisted — safe, prefer these:**
- `git rm` / `git rm --cached` — remove files through git (recoverable via history).
- `git mv` — rename/move through git.
- `git restore <path>` (single file), `git revert`, `git stash` (push) — reversible.
- Editing via the editor tools, `git add`, `git commit`, `git switch -c`.

If a genuinely destructive action seems unavoidable, **STOP and ask the operator first** — do not improvise around this rule.

## Syncing with the remote

“Sync with the remote” (or “sync”) is a two-way reconciliation, not a push-only operation and not merely a clean working tree.

1. Inventory local branches, worktrees, uncommitted changes, and relevant remote refs. Preserve all valid work.
2. Commit or safely stash the intended work before integrating incoming changes.
3. Run `git fetch --all --prune`.
4. Integrate upstream with merge-based history; do not rebase shared work.
5. Resolve conflicts semantically: do not merely choose ours or theirs. Merge the intended behaviors conceptually.
6. Grep for `<<<<<<<`, `=======`, and `>>>>>>>`; repeat review and tests until no conflict markers remain.
7. Run the complete applicable test and integration matrix.
8. Push the feature branch, merge through a green PR, and verify local and remote `main` contain the same intended commits.

Never `git rebase` or force-push to perform a shared synchronization.
