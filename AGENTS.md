# Agent guidelines — sonus-auris-site.web

Marketing website and public legal/store-review surface for Sonus Auris.

## Instruction discovery

- Resolve the real path of `$PWD`, walk its ancestors through the filesystem root, and load every readable lowercase `agents.md` in root-to-leaf order.
- Do not search sibling directories. Deduplicate canonical paths, detect symlink cycles, and report unreadable instruction files.
- `AGENTS.md`, `.claude/CLAUDE.md`, `.gemini/GEMINI.md`, and `.openai/AGENTS.md` are compatibility pointers only; lowercase `agents.md` files are canonical.

## Linear mapping

- GitHub organization: `github.com/sonus-auris`.
- Linear project: `github.com/sonus-auris` in the Denman workspace.
- Locate or create the matching Linear issue before substantial work, and record PR links, tests, blockers, and remaining work there.

## Public-site invariants

- Treat privacy, account-deletion, contact, download, SEO, accessibility, and store-review content as release-critical user-facing behavior.
- Never reintroduce placeholder legal identities, contact addresses, store URLs, or deletion instructions.
- Run the generated-site verifier and deployed browser/compliance suites after changes to public routes, metadata, downloads, or legal copy.
- Keep production publication gated through the existing GitHub Pages verification workflow; do not bypass it with manual artifact edits.

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
2. Commit or safely stash intended work before integrating incoming changes.
3. Run `git fetch --all --prune`.
4. Integrate upstream with merge-based history; do not rebase shared work.
5. Resolve conflicts semantically: do not merely choose ours or theirs. Merge the intended behaviors conceptually.
6. Grep for `<<<<<<<`, `=======`, and `>>>>>>>`; repeat review and tests until no conflict markers remain.
7. Run `npm ci` and `npm run check`, plus relevant deployed-site tests for public behavior.
8. Push the feature branch, merge through a green PR, and verify local and remote `main` contain the same intended commits.

Never `git rebase` or force-push to perform a shared synchronization.
