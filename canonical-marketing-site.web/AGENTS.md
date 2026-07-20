# Agent guidelines — canonical-marketing-site.web

[Astro](https://astro.build) marketing site for **canonical.cloud** (SOC 2,
FedRAMP & HIPAA compliance audits). Static build; served in production by
`canonical-web-server.rs` out of its `static/` dir.

## Layout

- `src/pages/index.astro` — the single landing page (hero, stats, services,
  process, frameworks, why, testimonial, CTA).
- `src/layouts/BaseLayout.astro` — `<head>` metadata, nav, footer.
- `src/styles/global.css` — design tokens + base styles.
- `tests/` — `node --test` specs (static contract + Playwright + Puppeteer e2e).

## Working here

- Enter the dev shell: `direnv allow` (or `nix develop ./.nix`, or `./shell`).
- `npm run dev` — local dev server (http://localhost:4321).
- `npm run build` — production build to `dist/`.
- `npm run preview` — serve the built `dist/`.

## Tests

- `npm test` — fast static-contract specs (no browser); the CI gate.
- `npm run test:browser` — builds, then runs the Playwright **and** Puppeteer
  e2e specs (both runners) via `node --test`. Needs a local Chrome; set
  `PUPPETEER_EXECUTABLE_PATH` / `PLAYWRIGHT_CHROMIUM` or let Puppeteer download
  one. Best-effort in CI.
- Point the browser suite at an already-running site with
  `CANONICAL_SITE_TEST_URL=http://localhost:4321`.

## Command safety

Agents working in this repo must **not** run destructive shell commands.

**Blacklisted (never run):** `rm`, `rm -rf`, `rmdir`, `dd`, `mkfs`, `shred`,
`truncate`, `> file` truncation, `find … -delete`, `git clean -fdx`,
`git reset --hard` on shared branches, `git push --force` to `main`, and any
`sudo`-prefixed or disk/format command.

**Whitelisted (prefer these):** `git rm` and `git mv` to delete/move tracked
files (they stay reviewable and reversible via history), `git restore` /
`git revert` to undo, and creating files under the gitignored `tmp/` for scratch
work. When something genuinely must be removed, stage it with `git rm` and let a
human review the commit — do not delete files out-of-band with `rm`.

## Git worktrees

Create git worktrees under `tmp/worktrees/` (e.g. `tmp/worktrees/<branch>`).
`tmp/` is gitignored, so worktree checkouts never show up as untracked files or
get committed by accident.

## Conventions

- Keep links base-aware: use `import.meta.env.BASE_URL` (see `BaseLayout.astro`)
  so the site works both at `/` locally and behind a gateway prefix.
- Any content change to nav labels, service cards, or framework names must keep
  the `tests/site-contract.test.mjs` assertions in sync (or update them).
- No secrets in the repo.

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
