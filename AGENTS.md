# Agent guidelines — akrion-web-server.rs

Rust (axum + maud) web portal for **Akrion Sim**. Renders htmx pages, a portal
UI, WebSocket stats, and Supabase browser login. `akrion-backend.rs` owns the
realtime game/simulation routes; this repo owns the web pages.

## Layout

- `src/main.rs` — thin process bootstrap and bind lifecycle.
- `src/app.rs` — app state, public browser config, env helpers, asset paths.
- `src/database.rs` — optional SeaORM/Postgres pool and readiness.
- `src/routes.rs` — axum routes, htmx partials, the WebSocket stream.
- `src/shutdown.rs` — graceful process signals.
- `src/telemetry.rs` — Loki-ready JSON logs and OTLP traces/metrics.
- `src/views.rs` — maud page templates and UI fragments.
- `src/data.rs` — dashboard stats and portal rows.
- `assets/` — `app.css`, `app.js`, `theme.js`, emblem.
- `e2e/` — Playwright + Puppeteer browser tests (`node --test`), isolated from cargo.

## Endpoints

- `GET /` — home page (`title` "Akrion Sim", hero, Home/Portal nav).
- `GET /healthz` — bare `ok` liveness probe.
- `GET /portal`, `GET /partials/*`, `GET /ws/portal` (WebSocket), `GET /config`.
- `/assets/*` — static files.

## Working here

- Enter the dev shell: `direnv allow` (or `nix develop ./.nix`, or `./shell`).
- Run: `PORT=8124 AKRION_BACKEND_URL=http://127.0.0.1:8113 cargo run`, open
  http://127.0.0.1:8124.
- Format + lint + test before pushing:
  ```sh
  cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
  ```
- Browser e2e: `cargo build` then `cd e2e && npm ci && npm test` (boots the
  built binary on an ephemeral port and drives it with Playwright and Puppeteer).

## Command safety — STRICT (all agents MUST follow)

Never run destructive or irreversible shell commands. To remove or move files,
**always go through git** so the change is tracked and recoverable.

**Blacklisted — do NOT run:**
- `rm`, `rm -rf`, `rmdir`, `unlink` — never delete via raw `rm`.
- bulk / indirect deletion: `find … -delete`, `find … -exec rm …`, `xargs rm` — no bypasses of the `rm` ban.
- raw `mv` of tracked files; truncating a tracked file with `>` or `truncate`.
- `git reset --hard`, `git clean -fdx`, `git checkout -- .` / `git restore .` mass-discard.
- `git stash drop` / `git stash clear`, `git branch -D`, `git tag -d` — destroy unmerged work / refs; not on shared branches unless the operator explicitly asks.
- `git push --force` / history rewrites on shared branches (esp. `main`).
- `dd`, `mkfs`, `shred`, recursive `chmod -R` / `chown -R` on broad paths, fork bombs.

**Whitelisted — safe, prefer these:**
- `git rm` / `git rm --cached` — remove files through git (recoverable via history).
- `git mv` — rename/move through git.
- `git restore <path>` (single file), `git revert`, `git stash` (push) — reversible.
- Editing via the editor tools, `git add`, `git commit`, `git switch -c`.

If a genuinely destructive action seems unavoidable, **STOP and ask the operator
first** — do not improvise around this rule.

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
