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

"Sync with the remote" (or just "sync") is **bidirectional and always contacts
the remote** — it fetches *and* pushes, never push-only. A clean local working
tree does **not** by itself mean "synced": a sync is not finished until local
and the remote have exchanged commits in both directions.

How to sync:

1. `git fetch --all --prune` — always safe; it only updates remote-tracking
   refs and never touches your working tree, so run it any time.
2. Make the working tree **clean before you pull/merge**: `git add` +
   `git commit` your work (or `git stash`). **Only `git pull` / `git merge`
   when the tree is not dirty** — pulling into a dirty tree makes git refuse
   the merge or tangle uncommitted edits with the incoming commits.
3. `git pull` (which fetches + merges) — or `git merge` the upstream tracking
   branch — to integrate the remote's commits into your now-clean branch.
4. `git push` — publish your commits so the remote has them too.

Integrate with **`git merge`** / **`git pull`** (which merges). **Never
`git rebase`** to sync — it rewrites history and breaks shared branches.
