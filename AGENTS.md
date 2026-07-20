# Global Codex Agent Instructions

## Command Whitelist (Safe - Always Allowed)

The following commands are safe to use without restriction:
- `find` - locate files and directories
- `grep` / `git grep` - search file contents
- `curl` - make HTTP requests
- `read` - read input / variables
- `cat` - display file contents
- `bash` - run shell scripts (but NEVER use `bash` to invoke blacklisted commands below)
- `sh` - run shell scripts (but NEVER use `sh` to invoke blacklisted commands below)

## Command Blacklist (NEVER Use)

The following commands are permanently banned:
- `rm` - use `git rm` instead for tracked files
- `sed` - causes codebase corruption; use targeted in-place edits
- `mv` - use `git mv` instead for tracked files

These bans also apply when invoked via bash or sh:
- `bash -c 'rm ...'` - BANNED
- `bash -c 'sed ...'` - BANNED
- `bash -c 'mv ...'` - BANNED
- `sh -c 'rm ...'` - BANNED
- `sh -c 'sed ...'` - BANNED
- `sh -c 'mv ...'` - BANNED

## Single-File Edit Policy

1. Never batch edits across multiple files in one operation.
2. Always use `grep` or `git grep` to locate target lines before editing.
3. Edit files in place one file at a time, then validate before moving to the next file.
4. Never use the `sed` CLI command.

## Task Completion Git Policy

When done with a task, add all changes, commit them, and push them to the configured remote. If the remote has new commits, pull them in, resolve any conflicts semantically, commit the resolution if needed, and re-push.

## Syncing with the remote

"Sync with the remote" (or just "sync") is **bidirectional and always contacts
the remote** — it pulls *and* pushes. It is never push-only, and a clean local
working tree does **not** by itself mean "synced": a sync is not finished until
local and the remote have exchanged commits in both directions.

The steps for a sync:

1. `git fetch --all --prune` — see what the remote has.
2. `git pull` (which merges) — or `git merge` the upstream tracking branch —
   to integrate the remote's commits into your local branch **first**.
3. `git add` / `git commit` any local work.
4. `git push` — publish your commits.

Always integrate with **`git merge`** (and plain `git pull`, which merges).
**Do not `git rebase`** to sync — rebasing rewrites history and breaks shared
branches; keep the merge history instead.
