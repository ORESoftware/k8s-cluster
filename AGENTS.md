# Agent guidelines — sonus-auris-backend.rs

Rust backend for Sonus Auris — the audio-dashcam server.

## Command safety — STRICT (all agents MUST follow)

Never run destructive or irreversible shell commands. To remove or move files,
**always go through git** so the change is tracked and recoverable.

**Blacklisted — do NOT run:**
- `rm`, `rm -rf`, `rmdir`, `unlink` — never delete via raw `rm`.
- bulk / indirect deletion: `find … -delete`, `find … -exec rm …`, `xargs rm` — no bypasses of the `rm` ban.
- raw `mv` of tracked files; truncating a tracked file with `>` or `truncate`.
- `git reset --hard`, `git clean -fdx`, `git checkout -- .` / `git restore .` mass-discard.
- `git stash drop` / `git stash clear`, `git branch -D`, `git tag -d` — destroy unmerged work / refs; not on shared branches unless the operator explicitly asks.
- `git push --force` / history rewrites on shared branches (esp. `main`).
- `dd`, `mkfs`, `shred`, recursive `chmod -R` / `chown -R` on broad paths, fork bombs.

**Whitelisted — safe, prefer these:**
- `git rm` / `git rm --cached` — remove files through git (recoverable via history).
- `git mv` — rename/move through git.
- `git restore <path>` (single file), `git revert`, `git stash` (push) — reversible.
- Editing via the editor tools, `git add`, `git commit`, `git switch -c`.

If a genuinely destructive action seems unavoidable, **STOP and ask the operator
first** — do not improvise around this rule.

## GitHub ↔ Linear coordination

- GitHub org: `sonus-auris` — https://github.com/sonus-auris
- Linear workspace/team: `denman` / `Denman` (`DEN`)
- Linear team ID: `eb8ab169-5afe-4b6f-9cab-3f2aa3e887dc`
- Linear project: `github.com/sonus-auris`
- Linear project ID: `40905103-ae88-4186-9cff-858b7b9384d2`
- Linear project URL: https://linear.app/denman/project/githubcomsonus-auris-a557165528ef

All repositories in this org map to that Linear project unless a nested `AGENTS.md` explicitly says otherwise. Before non-trivial work, search for and reuse an existing issue; otherwise create one in team `DEN` and this project with repository links, context, scope, acceptance criteria, risks, and validation. Link branches/commits/PRs to the issue when practical, keep status and blockers current, and file deferred or incomplete work before ending. Never commit credentials or API tokens.

## Syncing with remote — authoritative meaning

“Sync with remote,” “sync the org,” “sync all repos,” or “make main up to date” means the complete process below, not only `git pull` in this checkout.

1. Enumerate every public/private repository in `sonus-auris`, including missing local clones; identify every checkout/worktree and explicitly list archived/read-only exceptions.
2. Preserve all work: inspect status, untracked files, stashes, local/remote branches, tracking refs, and `git worktree list`. Never hard-reset, blanket-restore, delete branches/worktrees, or force-push to discard work.
3. In every writable repo run `git fetch --all --prune --tags`, ensure local `main` tracks `origin/main`, fast-forward when possible, and deliberately reconcile divergent main histories.
4. Inspect every branch and worktree for commits or intended changes absent from `main`. Integrate all valuable unique work into `main` by semantic merge, cherry-pick, or careful reimplementation; do not blindly merge obsolete/generated history merely for ancestry.
5. Resolve conflicts conceptually after understanding both sides, history, callers, schemas, and tests. Never globally choose “ours”/“theirs” or merely remove marker lines.
6. Run applicable formatting, lint, tests, and builds. Run `git diff --check` and scan the whole tree with `rg -n --hidden -g '!.git' '^(<<<<<<< .+|=======|>>>>>>> .+)$' .` (or equivalent recursive `grep`); investigate every match.
7. Review for secrets/unwanted artifacts, then `git add -A`, commit accurately, and publish to `origin/main`. If protected, merge an integration PR and verify the final commit is on `origin/main`. Never force-push `main` without explicit owner authorization.
8. Fetch and repeat from step 1 until every writable repo is clean, local `main` equals `origin/main`, all intended branch/worktree work is represented in `main`, checks pass or a Linear blocker exists, and marker scans are clean.

Do not claim completion when any repo, branch, worktree, failure, or read-only exception was silently skipped; report the exact final state and remaining Linear/PR links.