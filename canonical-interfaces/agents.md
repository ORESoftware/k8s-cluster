# Agent guidelines — canonical-interfaces

Typed-IO source of truth for the canonical.cloud API + compliance store. JSON
Schema in `schema/` is generated into per-language adapters under `generated/`.

## Layout

- `schema/*.schema.json` — the source of truth (indexed by `schema/index.json`).
- `sql/schema.sql` — canonical Postgres schema for stored entities.
- `src/generate.mjs` — the generator (schema → TS/Rust/Python/Go).
- `src/generate.test.mjs` — generator self-tests + `--check`.
- `generated/<lang>/` — **adapters only; never hand-edit.**

## Working here

- Enter the dev shell: `direnv allow` (or `nix develop ./.nix`, or `./shell`).
- Add a type: add a PascalCase `$def` with exact lowerCamelCase fields for API
  wire payloads or snake_case fields for the established compliance domain
  (new files must be listed in `schema/index.json`), then:
  ```sh
  npm run generate     # rewrite generated/<lang>
  npm test             # self-tests + verify generated/ is up to date
  ```
- Commit the regenerated `generated/` alongside the schema change — CI runs
  `npm run check` and fails if `generated/` is stale.
- Keep `sql/schema.sql` field names in sync with the JSON Schema.

## Command safety

Agents working in this repo must **not** run destructive shell commands.

**Blacklisted (never run):** `rm`, `rm -rf`, `rmdir`, `dd`, `mkfs`, `shred`,
`truncate`, `> file` truncation, `find … -delete`, `git clean -fdx`,
`git reset --hard` on shared branches, `git push --force` to `main`, and any
`sudo`-prefixed or disk/format command. Never hand-delete files in `generated/`
— regenerate instead.

**Whitelisted (prefer these):** `git rm` and `git mv` to delete/move tracked
files, `git restore` / `git revert` to undo, and scratch under the gitignored
`tmp/`. When something must be removed, stage it with `git rm` for review — never
`rm`.

## Git worktrees

`tmp/worktrees/` is reserved for a worktree only when a human explicitly instructs its use; `tmp/` is gitignored.

<!-- ore-primary-branch-policy:begin -->
## Primary branch and concurrent-agent policy

This policy overrides generic feature-branch and worktree defaults for agent tooling.

- Highly prefer an existing primary branch, in this order: `main`, `dev`, then `master`.
- Work directly on the selected primary branch even when other agents are active. Use another branch only when a human or a repository-specific release process explicitly requires it.
- Never create or use a Git worktree unless a human explicitly instructs you to do so for the current task. Concurrency alone is not permission to use a worktree.
- Concurrent agents must coordinate repository and file ownership through the available agent communication channel, keep edits scoped, inspect live state before each write, and hand off cleanly. Coordinate instead of isolating routine work in worktrees.
- Preserve unrelated in-progress changes and never overwrite another agent's work. If safe ownership of overlapping files cannot be established, pause that overlapping edit and coordinate before continuing.
<!-- ore-primary-branch-policy:end -->
