# Agent guidelines — canonical-monorepo

Git superproject for the canonical-cloud repos. Each app lives in its own repo
and is tracked here as a submodule under `apps/`:

- `apps/canonical-backend.rs` — Rust (axum) service
- `apps/canonical-frontend` — Astro static site

## Working here

- Clone/refresh with submodules: `git submodule update --init --recursive`.
- Change app code **inside the submodule** (`apps/<app>`), commit and push there
  first, then update the pin here with `scripts/pin-submodules.sh main`.
- The superproject only ever stores submodule *pins* (gitlinks) + shared config
  (CI, docs, scripts). Don't vendor app source directly into the superproject.
- `./build.sh` builds the frontend and serves it via the backend for a full
  local stack.
- `npm test` runs the `node --test` contract specs that keep the submodule
  wiring, README, and scripts honest.

## Command safety

Agents working in this repo must **not** run destructive shell commands.

**Blacklisted (never run):** `rm`, `rm -rf`, `rmdir`, `dd`, `mkfs`, `shred`,
`truncate`, `> file` truncation, `find … -delete`, `git clean -fdx`,
`git reset --hard` on shared branches, `git submodule deinit`,
`git push --force` to `main`, and any `sudo`-prefixed or disk/format command.
Deleting a submodule checkout with `rm -rf apps/<app>` is especially forbidden —
it silently corrupts the superproject's gitlink state.

**Whitelisted (prefer these):** `git rm` and `git mv` to delete/move tracked
files (they stay reviewable and reversible via history), `git restore` /
`git revert` to undo, `git submodule update` to reconcile checkouts, and the
`scripts/*.sh` helpers (which are intentionally push-free and offer `--dry-run`
/ `--allow-dirty` guards). When something must be removed, stage it with
`git rm` and let a human review the commit — never delete files with `rm`.

## Git worktrees

Create git worktrees under `tmp/worktrees/` (e.g. `tmp/worktrees/<branch>`).
`tmp/` is gitignored, so worktree checkouts never show up as untracked files or
get committed by accident.

## Scripts

- `scripts/pin-submodules.sh <branch>` — pin every submodule to a branch tip.
- `scripts/checkout-feature-branch.sh <branch>` — switch superproject + every
  submodule to the same feature branch.
- `scripts/audit-repo-state.sh` — check for conflict markers, tracked secrets,
  and submodule/readme drift.

All three refuse to `git push` and validate branch names; the mutating ones
support `--dry-run`.
