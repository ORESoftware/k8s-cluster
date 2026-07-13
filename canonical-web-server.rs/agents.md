# Agent guidelines — canonical-web-server.rs

Rust sMASH application server for **canonical.cloud**: Supabase Auth/Postgres,
Maud, Axum, SeaORM, HTMX, REST, WebSockets, and an IndexedDB sync client. The
static marketing site lives in `canonical-marketing-site.web` and is only the
server's final filesystem fallback.

## Layout

- `src/main.rs` — configuration/bootstrap only.
- `src/auth/` — Supabase and opaque application sessions.
- `src/db/` — SeaORM entities/migration and user-context transactions.
- `src/routes/`, `src/views/`, `src/ws/` — HTTP, Maud/HTMX, and WebSockets.
- `src/sync/` — versioned/idempotent REST sync protocol.
- `client/` — TypeScript/IndexedDB optimistic client and HTMX bundle.
- `tests/app.rs` — router, auth, sync, and real WebSocket integration tests.
- `static/` — uncommitted marketing build selected by `STATIC_DIR`.

## Working here

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
npm test --prefix client
npm run typecheck --prefix client
npm run build --prefix client
```

Run locally with an ignored environment file based on `.env.example`. For
production, migrate with a privileged database identity and run with a separate
least-privilege, non-`BYPASSRLS` role.

## Security invariants

- `/healthz` remains dependency-free; `/readyz` may probe the database.
- Static marketing fallback is always last. `/api`, `/auth`, and `/app` must
  retain their own 404 behavior.
- Never log passwords, cookies, Authorization headers, Supabase tokens, DB
  URLs, encryption keys, or upstream Auth response bodies.
- Browser sessions are opaque. Supabase access/refresh tokens remain encrypted
  server-side, and refresh rotation stays under a row lock.
- Cookie-authenticated unsafe HTTP requests require both exact Origin and CSRF.
  Browser WebSockets require exact Origin before upgrade. Never place tokens in
  WebSocket URLs or subprotocols.
- Every user-owned repository method derives ownership from verified auth and
  runs in a user-context transaction. Never accept `owner_id` from a payload.
- IndexedDB is optimistic, not authoritative. WebSocket messages only wake a
  REST pull; they never advance the durable cursor.
- Versions are decimal strings over the wire; server writes use exact CAS.
  Tombstoned IDs are not resurrected, and mutation IDs are immutable.
- New sync kinds require bounded validation and explicit authorization; do not
  turn the protocol into unrestricted arbitrary JSON storage.
- No secrets in the repository. Use only a Supabase publishable key for normal
  Auth flows; do not add a service-role/secret key casually.

## Command safety

Agents working in this repo must **not** run destructive shell commands.

**Blacklisted (never run):** `rm`, `rm -rf`, `rmdir`, `dd`, `mkfs`, `shred`,
`truncate`, `> file` truncation, `find … -delete`, `git clean -fdx`,
`git reset --hard` on shared branches, `git push --force` to `main`, and any
`sudo`-prefixed or disk/format command.

**Whitelisted (prefer these):** `git rm` and `git mv` for tracked removals and
moves, `git restore` / `git revert` to undo, and files under ignored
`tmp/worktrees/` for scratch work. Let a human review staged removals.

## Git worktrees

Create worktrees only under `tmp/worktrees/<branch>`; `tmp/` is ignored.
