# Agent guidelines — sonus-auris-monorepo

Git superproject for the Sonus Auris repositories — each app is a git submodule under `apps/`, plus a Nix dev shell (`nix develop`).

## Submodules

Every app lives under `apps/<name>` as a git submodule pinned to a commit on its
`main`. Never `rm -rf` a submodule directory. To repoint a submodule, commit and
push **inside** that submodule, then stage the updated gitlink here with
`git add apps/<name>`. Put scratch checkouts / worktrees under `tmp/` (gitignored).

## Command safety

Prefer reversible, git-tracked operations. Route every file deletion and move
through git so it lands in history and stays recoverable. Never run a command
that irreversibly destroys files, history, or system state.

### Forbidden — never run

- **Raw deletion:** `rm`, `rm -rf`, `rmdir`, `unlink`, `find … -delete`, `find … -exec rm …`
- **Clobbering:** `mv`/`cp` over an existing path, `> file`, `>|`, `truncate`, `dd`, `shred`, `mkfs`
- **Discarding local work:** `git clean -f`/`-fdx`, `git reset --hard`, `git checkout -- <path>`, `git restore <path>`, `git stash drop`/`clear`
- **Rewriting shared history:** `git push -f`/`--force`/`--force-with-lease`, `git rebase`, `git commit --amend`, `git filter-branch`, `git reflog expire`, `git gc --prune=now` on anything already pushed
- **System / privilege:** `sudo …`, `chmod -R`, `chown -R`, `kill -9`, `pkill`, `killall`
- **Remote-to-shell piping:** `curl … | sh`, `wget … | bash`

### Allowed — use these instead

- `git rm <path>` instead of `rm` — stages the deletion; fully recoverable from history
- `git mv <old> <new>` instead of `mv` — a tracked rename
- `git revert`, a fresh branch, or `git restore --staged <path>` (unstage only) to undo safely
- Read-only inspection is always fine: `ls`, `cat`, `git status`, `git diff`, `git log`, `rg`, `grep`
- Branch switching is fine (`git switch <branch>`, `git checkout <branch>`); only the pathspec forms that discard changes are forbidden

If a task appears to require a forbidden command, stop and ask first.
