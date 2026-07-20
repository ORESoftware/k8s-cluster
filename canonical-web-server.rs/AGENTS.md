# Agent guidelines — canonical-web-server.rs

Rust sMASH application server for **canonical.cloud**: Supabase Auth/Postgres,
Maud, Axum, SeaORM, HTMX, REST, WebSockets, and an IndexedDB sync client. The
static marketing site lives in `canonical-marketing-site.web` and is only the
server's final filesystem fallback.

## Layout

- `crates/canonical-auth/` — transport-neutral Supabase Auth client and
  verified credential/identity types. It must reject secret/service-role keys.
- `crates/canonical-config/` — process-specific web, migration, and revoker
  configuration with redacted debug output.
- `crates/canonical-session/` — opaque session crypto, refresh rotation,
  durable logout reconciliation, and local bearer revocation.
- `crates/canonical-store/` — SeaORM entities/migrations and exact
  user/admin/revoker transaction boundaries.
- `services/canonical-session-revoker/` — no-ingress worker. It must not
  depend on `canonical-web-server`, Axum, Maud, routes, or WebSockets.
- `src/main.rs` / `src/command.rs` — telemetry/bootstrap and explicit
  `serve` / `migrate` command dispatch only.
- `src/auth/` — Axum authentication extractors, CSRF/Origin checks, and
  bounded login throttling.
- `src/app.rs`, `src/server.rs` — state/router assembly and network lifecycle.
- `src/database.rs` — SeaORM pool policy and the explicit migration command.
- `src/telemetry.rs` — stdout JSON logs plus OTLP HTTP traces/metrics.
- `src/routes/`, `src/views/`, `src/ws/` — HTTP, Maud/HTMX, and WebSockets.
- `src/sync/` — versioned/idempotent REST sync protocol.
- `client/` — TypeScript/IndexedDB optimistic client and HTMX bundle.
- `tests/app.rs` — router, auth, sync, and real WebSocket integration tests.
- `static/` — uncommitted marketing build selected by `STATIC_DIR`.

## Working here

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
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
  retain their own 404 behavior. `/admin` and `/api/admin` are reserved for a
  separately deployed admin origin and must never fall through to marketing.
- Never log passwords, cookies, Authorization headers, Supabase tokens, DB
  URLs, encryption keys, or upstream Auth response bodies.
- Browser sessions are opaque. Supabase access/refresh tokens remain encrypted
  at rest and are never exposed through the browser session cookie.
- A locally revoked Supabase `session_id` remains denied while its access JWT
  can still be valid. Upstream credential rejection is a dead-letter state,
  never a successful revocation.
- Cookie-authenticated unsafe HTTP requests require both exact Origin and CSRF.
  Browser WebSockets require exact Origin before upgrade. Never place tokens in
  WebSocket URLs or subprotocols.
- Every user-owned repository method derives ownership from verified auth and
  runs in a user-context transaction. Never accept `owner_id` from a payload.
- PostgreSQL web startup must verify the exact `canonical_web_server` login
  before migration or HTTP binding; never weaken this to a privilege heuristic.
- Production revoker code routes `web_session` DML through
  `begin_session_revocation_transaction` and never installs a customer claim.
  Its exact database login is non-owner, non-`BYPASSRLS`, non-inheritable, and
  has no application ingress.
  The transaction marker is an audit/accidental-use guard, not authorization;
  custom PostgreSQL settings are caller-settable, so the isolated exact role
  and its one-table grant are the security boundary.
- Admin capability functions require a freshly verified AAL2 identity and
  derive the actor from `auth.uid()`. Do not add an admin route, owner-RLS
  bypass, or admin credential to the customer web process.
- IndexedDB is optimistic, not authoritative. WebSocket messages only wake a
  REST pull; they never advance the durable cursor.
- Versions are decimal strings over the wire; server writes use exact CAS.
  Tombstoned IDs are not resurrected, and mutation IDs are immutable.
- New sync kinds require bounded validation and explicit authorization; do not
  turn the protocol into unrestricted arbitrary JSON storage.
- No secrets in the repository. The customer web and revoker processes accept
  only a Supabase publishable/legacy-anon key. A secret/service-role key, if a
  future admin capability genuinely needs one, belongs only in that separate
  service's deployment environment.

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

## Syncing with the remote

"Sync with the remote" (or just "sync") is **bidirectional and always contacts
the remote** — it pulls *and* pushes. It is never push-only, and a clean local
working tree does **not** by itself mean "synced": a sync is not finished until
local and the remote have exchanged commits in both directions.

The steps for a sync:

1. `git fetch --all --prune` — see what the remote has.
2. `git pull` (which merges) — or `git merge` the upstream tracking branch —
   to integrate the remote's commits into your local branch **first**.
3. `git add` / `git commit` any local work.
4. `git push` — publish your commits.

Always integrate with **`git merge`** (and plain `git pull`, which merges).
**Do not `git rebase`** to sync — rebasing rewrites history and breaks shared
branches; keep the merge history instead.
