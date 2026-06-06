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
