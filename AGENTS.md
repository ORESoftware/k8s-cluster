# Agent instructions

## Repository and tracking

- Repository: `github.com/fanwaave/push-notification-server.rs`
- Legacy transfer source: `github.com/ORESoftware/push-notification-server.rs` (redirect only; do not recreate)
- Linear project: `github.com/ORESoftware/push-notification-server.rs` (legacy tracker name retained during the repository transfer)
- Parent implementation issue: `DEN-257`
- Bootstrap: `DEN-259`
- Provider extraction: `DEN-261`
- Cluster submodule/deployment: `DEN-263`
- Supabase integration: `DEN-264`
- Reliability/security/observability: `DEN-265`

## Git workflow

- Work from feature branches cut from `main` and use pull requests.
- Avoid git rebase in favor of git merge.
- Sync with remote before and after material work.
- Resolve git conflicts semantically: do not merely pick one side. Merge the intended concepts and behavior.
- After resolving conflicts, grep the entire worktree for conflict markers (`<<<<<<<`, `=======`, `>>>>>>>`), review from the top, and repeat as necessary.
- Never commit secrets, production device tokens, Web Push capability URLs, or provider private keys.

## Nested instructions

Before editing, walk upward from `$PWD` to the filesystem root and apply every relevant `AGENTS.md`, from broadest to most specific.
