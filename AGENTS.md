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
