# Agent guidelines — canonical-interfaces

Typed-IO source of truth for the canonical.cloud API + compliance store. JSON
Schema in `schema/` is generated into per-language adapters under `generated/`.

## Layout

- `schema/*.schema.json` — the source of truth (indexed by `schema/index.json`).
- `sql/schema.sql` — canonical Postgres schema for stored entities.
- `src/generate.mjs` — the generator (schema → TS/Rust/Python/Go).
- `src/generate.test.mjs` — generator self-tests + `--check`.
- `generated/<lang>/` — **adapters only; never hand-edit.**
- `tests/interface-wasm-browser/` — hermetic Chromium contract for the generated
  Rust/WASM package.
- `.ci/` — exact TypeScript compiler lock plus reviewed dependency-lock hashes.

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

## Generated Rust/WASM boundary

- `generated/rust-wasm/Cargo.lock` is generated dependency state and is the one
  tracked-lock exception under `generated/`. Regenerate it with Cargo; never
  edit package versions or checksums by hand.
- `.ci/package-lock.json` locks the declaration-check compiler. Install it only
  with `npm ci --ignore-scripts --prefix .ci`; do not replace it with `npx` or a
  mutable `npm install` in CI.
- `.ci/locks.sha256` records the reviewed lock artifacts. Any intentional lock
  refresh must update both lockfiles and their hashes in one reviewed change.
- CI pins Rust 1.95 and exact locked `wasm-pack 0.15.0`, builds with Cargo's
  committed lock, proves packaging did not rewrite that lock, and type-checks
  declarations with exact TypeScript 5.9.3.
- Never downgrade or float the WASM package builder merely to bypass an upstream
  install warning. Change the exact version only with the full declaration and
  Chromium contracts on the same reviewed head.
- The generated browser package is declaration-oriented. Do not add runtime
  network, mutation, storage, cookie, WebSocket, beacon, or probe-execution
  APIs merely to make a browser test interesting.
- Browser certification must initialize the actual wasm-pack output in a real
  Chromium process, serve loopback files only, block external DNS, verify
  representative declarations, and fail on any unexpected-origin resource.
- A server-side Cargo build or `.d.ts` grep alone is not browser evidence.

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

<!-- ore-primary-branch-policy:begin -->
## Primary branch and concurrent-agent policy

This policy overrides generic feature-branch and worktree defaults for agent tooling.

- Highly prefer an existing primary branch, in this order: `main`, `dev`, then `master`.
- Work directly on the selected primary branch even when other agents are active. Use another branch only when a human or a repository-specific release process explicitly requires it.
- Never create or use a Git worktree unless a human explicitly instructs you to do so for the current task. Concurrency alone is not permission to use a worktree.
- Concurrent agents must coordinate repository and file ownership through the available agent communication channel, keep edits scoped, inspect live state before each write, and hand off cleanly. Coordinate instead of isolating routine work in worktrees.
- Preserve unrelated in-progress changes and never overwrite another agent's work. If safe ownership of overlapping files cannot be established, pause that overlapping edit and coordinate before continuing.
<!-- ore-primary-branch-policy:end -->
