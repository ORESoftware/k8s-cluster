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
