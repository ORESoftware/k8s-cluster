# Agent Rules

Rules for AI agents (and humans) working in this repository.

## Forbidden — destructive operations

Never run, script, or suggest any of the following here:

- `rm` / `rm -rf` on tracked or untracked files (stage removals with `git rm` on a reviewed branch instead)
- `git rebase` (interactive or otherwise)
- `git reset` (any mode — `--soft`, `--mixed`, `--hard`)
- `git push --force` / `--force-with-lease` / `--force-if-includes`
- `git filter-repo`, `git filter-branch`, BFG, or any history-rewriting tool
- `git clean`
- `git checkout -- <path>` / `git restore` that discards uncommitted work
- deleting branches or tags (local `-D` or remote)
- amending commits that have been pushed

## Required workflow

- History is append-only. Fix mistakes with a new commit or `git revert` — never by rewriting.
- Changes land on `main` via feature branches; keep commits small and reviewable.
- **This repository is the source of truth.** The copy vendored into
  `ORESoftware/k8s-cluster` (under `remote/deployments/daedalus-web-server-rs`) is a
  *secondary* submodule checkout — after merging here, bump the submodule pointer
  there. Do not edit the vendored copy directly.

## Build context

Path dependencies (`../../libs/telemetry-rs`, `../../libs/pg-defs/...`) resolve only
when this repo is checked out at its `remote/deployments/` path inside the
`k8s-cluster` superproject. Full builds happen there; standalone CI is limited to
hygiene and format checks by design.

## Database & auth invariants — do not weaken

- **Schema source of truth is elsewhere.** The `daedalus` tables live in the shared
  pg-defs contract (`remote/libs/pg-defs/schema/schema.sql`), not in this repo.
  Change columns there and regenerate; never hand-edit the generated SeaORM adapter.
  Migrate with `scripts/dpm.sh`, never at boot.
- **No RLS on this database.** It is Amazon RDS, not Supabase. The server is the
  only authorization boundary. Every query MUST filter by the verified operator
  email; a query without that filter is a data leak.
- **Owner identity comes from the verified token, never from request input.**
- **The service-role key is never used by this server.** It bypasses RLS and is
  reserved for offline operator tooling (the daedalus-fab MCP server). A
  request-serving process acts as the calling user.
- **Auth fails closed.** Without both a verification key and a non-empty email
  allow-list, the `/v1` surface returns 503 rather than serving unauthenticated.
