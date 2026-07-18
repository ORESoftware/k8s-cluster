# canonical-web-server.rs

The Rust application server for **[canonical.cloud](https://canonical.cloud)**.
It is the dynamic half of the product; the sibling
[`canonical-marketing-site.web`](https://github.com/canonical-cloud/canonical-marketing-site.web)
repository remains the static Astro marketing site.

The server uses the sMASH stack:

- **s** — Supabase Auth and Supabase Postgres
- **M** — Maud server-rendered HTML
- **A** — Axum HTTP, REST, and WebSocket routing
- **S** — SeaORM entities, migrations, transactions, and RLS request context
- **H** — HTMX navigation and fragments

The authenticated application includes a TypeScript offline-first client. It
writes draft notes optimistically to IndexedDB, persists an idempotent outbox,
reconciles through the REST API, and treats WebSockets as invalidation hints.
Supabase Postgres remains authoritative; the browser never receives database
credentials or the server's Supabase token pair.

## Architecture

- `src/auth/` — Supabase GoTrue HTTP client, encrypted server-side token
  storage, opaque browser sessions, bearer/session extraction, CSRF and Origin
  checks.
- `src/db/` — SeaORM entities and a versioned migration for profiles, web
  sessions, records, commit-ordered sync clocks, change rows, and mutation
  receipts.
- `src/routes/` — probes, Maud/HTMX pages, versioned REST, and authenticated
  WebSocket upgrade handling.
- `src/sync/` — compare-and-swap mutations, durable idempotency, tombstones,
  owner-bound encrypted cursors, and pull pagination.
- `src/ws/` — owner-scoped in-process fanout plus a bounded PostgreSQL
  `LISTEN`/`NOTIFY` invalidation backplane for multi-instance deployments.
- `client/` — TypeScript, HTMX 2, IndexedDB (`idb`), Web Locks,
  BroadcastChannel, retry/backoff, conflict storage, and WebSocket reconnects.
- `static/` — optional built marketing site supplied through `STATIC_DIR`; it
  is always the final fallback and can never answer `/api`, `/auth`, or `/app`.

Only `draft_note` schema version 1 is accepted by the initial sync protocol.
This deliberately avoids an unrestricted arbitrary-JSON database API. Add a
new kind only with matching validation, authorization, schema, and merge rules.

## Routes

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/healthz` | Dependency-free liveness probe |
| `GET` | `/readyz` | Database readiness probe |
| `GET` | `/login` | Maud login page |
| `POST` | `/auth/login` | Supabase password login and opaque session creation |
| `POST` | `/auth/logout` | CSRF-protected local/Supabase logout |
| `GET` | `/app` | Authenticated Maud application shell |
| `GET` | `/app/fragments/session` | HTMX session fragment |
| `GET` | `/api/v1/{health,info,me}` | Versioned REST metadata/current user |
| `GET` | `/api/v1/sync/changes` | Incremental authoritative pull |
| `POST` | `/api/v1/sync/mutations` | Bounded idempotent mutation batch |
| `GET` (Upgrade) | `/ws` | Authenticated typed WebSocket invalidations |

`/api/health` and `/api/info` remain compatibility aliases. Unknown API and
application paths have JSON and HTML 404s respectively rather than falling
through to the marketing SPA.

## Multi-instance invalidations

Every committed sync mutation still wakes WebSockets attached to the current
process through a Tokio broadcast channel. On PostgreSQL, the same user-scoped
`{ version, sourceInstance, ownerId, cursor }` hint is also queued with
`pg_notify` inside the authoritative write transaction, so PostgreSQL releases
it only after commit. Each server instance owns a dedicated reconnecting
`LISTEN` connection, ignores its own notifications, validates a strict
512-byte payload bound, and relays remote hints through the same owner-filtered
hub. Listener failure does not stop HTTP service and reconnects with capped
backoff. Budget one additional PostgreSQL connection per server instance beyond
the SeaORM pool configured by `DATABASE_MAX_CONNECTIONS`.

These messages are deliberately disposable wake-ups: duplicates and missed
notifications are safe, message payloads never contain record data or auth
material, and clients must always use REST pull plus its encrypted durable
cursor to learn authoritative state.

## Supabase setup

Use a Supabase **publishable** key for user authentication. Do not configure a
secret/service-role key for this application path. The server calls GoTrue
directly, stores access/refresh tokens encrypted in `web_session`, and gives the
browser only a random `HttpOnly`, `Secure`, `SameSite=Lax`, `__Host-` cookie.
REST clients may send a Supabase bearer token; the first implementation verifies
it with the authenticating `/auth/v1/user` request rather than trusting decoded
claims.

`/auth/login` accepts only a 16 KiB form and has a bounded, normalized-account
throttle before it contacts Supabase. Configure a stronger trusted-client-IP
limit at the edge; this process intentionally does not trust forwarded-IP
headers. Production origins fail startup unless cookies are secure and
`__Host-` prefixed, and session lifetime is limited to 1–30 days.

Logout revokes the opaque local session first, then confirms Supabase sign-out.
An upstream outage creates a durable, encrypted-token-backed retry record. The
server's narrowly scoped worker retries it with a short database lease and
backoff, revokes expired local sessions, and prunes confirmed revocations after
seven days. It is not a general background-job privilege boundary.

Before adding an admin surface, create a separate origin/service with MFA or
reauthentication, an immutable audit trail, and a narrowly scoped deployment
credential. Do not add an owner-RLS bypass or a Supabase service-role key to the
customer-facing server.

The runtime `DATABASE_URL` must use a dedicated least-privilege Postgres role,
not `postgres`, the table owner, or a role with `BYPASSRLS`. User-owned SeaORM
operations install the validated user ID in transaction-local
`request.jwt.claim.sub` and `request.jwt.claims` settings; the migration enables
and forces owner RLS. Supavisor session mode or a direct connection is required
for the long-lived SeaORM pool and the dedicated PostgreSQL `LISTEN` connection;
transaction mode cannot preserve listener session state.

Use the migration-only command during deployment. It loads
`MIGRATION_DATABASE_URL` instead of the runtime `DATABASE_URL` and does not
construct the HTTP or Supabase Auth clients:

```sh
MIGRATION_DATABASE_URL='postgresql://PRIVILEGED_CONNECTION' \
  canonical-web-server migrate
psql "$MIGRATION_DATABASE_URL" \
  --file deploy/postgres/bootstrap_runtime_role.sql
canonical-web-server serve
```

Supabase schema changes are managed declaratively with
[dpm](https://github.com/declarative-migrations/declarative-postgres-migrate.rs):
`deploy/postgres/schema.sql` is the desired-state source of truth, and CI
proves the SeaORM migrations converge with it on every change (the
`declarative-schema` job). Against a live Supabase database, generate and
review a migration instead of hand-writing DDL — connect via the direct
connection or session pooler (5432), never the transaction pooler:

```sh
dpm diff   --source deploy/postgres/schema.sql --target "$MIGRATION_DATABASE_URL"            --shadow "$SHADOW_DATABASE_URL"      # review the SQL
dpm verify --source deploy/postgres/schema.sql --target "$MIGRATION_DATABASE_URL"            --shadow "$SHADOW_DATABASE_URL"      # rehearse on a shadow replica
dpm apply  --source deploy/postgres/schema.sql --target "$MIGRATION_DATABASE_URL"            --shadow "$SHADOW_DATABASE_URL"      # interactive confirm before writes
```

Destructive changes require dpm's two explicit consent flags and stay
commented out otherwise; grants are out of dpm's scope and stay in
`bootstrap_runtime_role.sql`.

The migrations are also proven against **CockroachDB** (v25.2+, which speaks
the Postgres wire protocol and supports forced RLS): the `cockroach-rls` CI
job applies the full chain to a single-node cluster and asserts the same
owner-isolation contract. Two documented divergences: CockroachDB validates
foreign keys with the inserting role's privileges (grant the app role SELECT
on `auth.users`), and it has no LISTEN/NOTIFY, so the WebSocket invalidation
backplane is Postgres-only — REST pull remains authoritative either way.

The bootstrap creates `canonical_web_server` as a non-owner,
non-`BYPASSRLS` login without a password and grants only the application's
current tables. Set its password or another authentication mechanism through
the deployment secret manager, never in this repository, then use that role in
`DATABASE_URL`. Re-run the bootstrap after future migrations change the table
allow-list. Keep `AUTO_MIGRATE=false` in production so the long-lived process
never needs owner credentials.

Copy `.env.example` to an ignored local environment file and replace every
placeholder. `APP_SESSION_ENCRYPTION_KEY` must be standard-base64 for exactly
32 random bytes and must be stored outside the database.

## Develop

```sh
direnv allow                       # or: nix develop ./.nix / ./shell
npm ci --prefix client
npm run build --prefix client
AUTO_MIGRATE=true cargo run
```

For the full local stack, build the sibling marketing site and set:

```sh
STATIC_DIR=../canonical-marketing-site.web/dist
```

SQLite is compiled in for focused local/integration tests. Production should
use Supabase Postgres with TLS.

## Verify

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
npm test --prefix client
npm run typecheck --prefix client
npm run build --prefix client
```

The Rust tests use in-memory SQLite and a fake Auth provider; they do not need
Supabase secrets. They cover route precedence, opaque login sessions, bearer
identity, sync idempotency/conflicts/pull, authentication-before-upgrade, and a
real WebSocket connection receiving a typed owner-scoped invalidation. Unit
tests also cover strict backplane decoding, size bounds, and source-instance
deduplication without requiring PostgreSQL. CI also runs a
PostgreSQL 17 fixture that migrates as an owner, connects as the dedicated
runtime login, proves no-claim and cross-user isolation, and verifies rolled-back
notifications are suppressed while committed hints are delivered. Run that
fixture locally only against a disposable loopback PostgreSQL cluster whose
`postgres` database is named in `TEST_POSTGRES_ADMIN_URL`:

```sh
TEST_POSTGRES_ADMIN_URL=postgresql://postgres:password@127.0.0.1:5432/postgres \
  cargo test --test postgres_rls -- --nocapture
```

## Container

The multi-stage image builds and tests the TypeScript client, builds the locked
Rust binary, and copies only the binary plus application assets into a
distroless non-root image. Supply the marketing build at `/app/static` and all
required runtime environment variables.

```sh
docker build -t canonical-web-server .
docker run --env-file .env.local -p 8081:8081 \
  -v "$PWD/static:/app/static:ro" canonical-web-server
```
