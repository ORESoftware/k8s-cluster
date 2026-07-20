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

Create git worktrees under `tmp/worktrees/`; `tmp/` is gitignored.

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
