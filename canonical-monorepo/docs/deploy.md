# Deploy

The canonical.cloud stack has a static Astro marketing site and a dynamic
sMASH application server. The Rust server serves Maud/HTMX pages, versioned
REST, authenticated WebSockets, and the built TypeScript/IndexedDB client. It
can also serve the prebuilt marketing site as its final static fallback. The
Maud shell declares the HTMX WebSocket extension on `/ws`; typed socket frames
wake the REST pull loop instead of directly mutating durable client state.

## Build all assets

```sh
git submodule update --init --recursive
./build.sh                      # Astro + app client + locked Rust release build
```

The script builds `canonical-marketing-site.web/dist`, verifies and builds
`canonical-web-server.rs/client/dist`, and builds the locked Rust release. Each
artifact stays in its owning submodule; the root `.env.example` points
`STATIC_DIR` and `APP_ASSET_DIR` at those outputs.

## 1. Single application-server process

Containerize the web server with `apps/canonical-web-server.rs/Dockerfile`.
The image builds the authenticated client into `/app/client` and runs as a
distroless nonroot user. Supply the marketing build at `/app/static` as a
read-only volume directly from the marketing submodule, or add it in a
higher-level release image.

```sh
docker build -t canonical-web-server apps/canonical-web-server.rs
docker run --env-file .env.runtime \
  -e STATIC_DIR=/app/static -e APP_ASSET_DIR=/app/client \
  -p 8081:8081 \
  -v "$PWD/apps/canonical-marketing-site.web/dist:/app/static:ro" \
  canonical-web-server
```

## 2. Split marketing site and application server

Serve the static site from the marketing site's nginx image
(`apps/canonical-marketing-site.web/Dockerfile`) and run the web server for
`/login`, `/auth/*`, `/app/*`, `/app-assets/*`, `/api/*`, `/ws`, `/healthz`, and
`/readyz`. The edge must preserve cookies and `Origin`, forward WebSocket
upgrade headers on `/ws`, and use timeouts suitable for long-lived connections.
Point the marketing-site build's base at the public path with `PUBLIC_BASE`.

## Required runtime configuration

Start from the root `.env.example` and inject real values with the deployment
platform rather than committing them. The server requires:

- `DATABASE_URL` for a dedicated least-privilege Supabase Postgres runtime role
  that neither owns the tables nor has `BYPASSRLS`;
- `MIGRATION_DATABASE_URL` and `MIGRATION_DATABASE_MAX_CONNECTIONS` only in the
  one-shot migration job, never in the long-lived server environment;
- `SUPABASE_URL` and a Supabase publishable key (never a secret/service-role
  key) for Auth;
- a unique 32-byte `APP_SESSION_ENCRYPTION_KEY`, base64 encoded, plus the exact
  public `APP_BASE_URL` and `APP_ALLOWED_ORIGINS`; and
- `STATIC_DIR` and `APP_ASSET_DIR` pointing at the two built browser asset
  directories.

Schema changes against Supabase are managed declaratively with
[dpm](https://github.com/declarative-migrations/declarative-postgres-migrate.rs).
The desired state lives in
`apps/canonical-web-server.rs/deploy/postgres/schema.sql`; the web server's CI
proves the SeaORM migrations converge with that file. To migrate a live
Supabase database, generate, rehearse, and apply a reviewed diff (direct
connection or session pooler only — never the transaction pooler):

```sh
dpm diff   --source apps/canonical-web-server.rs/deploy/postgres/schema.sql            --target "$MIGRATION_DATABASE_URL" --shadow "$SHADOW_DATABASE_URL"
dpm verify --source apps/canonical-web-server.rs/deploy/postgres/schema.sql            --target "$MIGRATION_DATABASE_URL" --shadow "$SHADOW_DATABASE_URL"
dpm apply  --source apps/canonical-web-server.rs/deploy/postgres/schema.sql            --target "$MIGRATION_DATABASE_URL" --shadow "$SHADOW_DATABASE_URL"
```

Destructive DDL stays commented out unless both dpm consent flags are given,
and grants remain the bootstrap script's job. For a fresh database you can
equally run the SeaORM migrations directly:

Run SeaORM migrations and establish the explicit runtime grants before serving:

```sh
set -a; source .env.migration; set +a
./apps/canonical-web-server.rs/target/release/canonical-web-server migrate
psql "$MIGRATION_DATABASE_URL" \
  --file apps/canonical-web-server.rs/deploy/postgres/bootstrap_runtime_role.sql
unset MIGRATION_DATABASE_URL MIGRATION_DATABASE_MAX_CONNECTIONS
```

The bootstrap creates/reasserts `canonical_web_server` as a non-owner,
non-`BYPASSRLS` login and grants only the current application tables. Configure
its password outside Git, use it in the runtime `DATABASE_URL`, and re-run the
bootstrap when future migrations change the table allow-list. Keep
`AUTO_MIGRATE=false` for `serve`.

Connection strings must use `sslmode=verify-full` (with the Supabase CA
bundle installed) — `sslmode=require` encrypts but does not authenticate the
server, so a spoofed endpoint could harvest the runtime credential.
Supavisor session mode or a direct TLS connection is required for the
long-lived SeaORM pool and PostgreSQL `LISTEN`/`NOTIFY` backplane; transaction
mode cannot preserve listener state. Budget one listener connection per server
instance in addition to `DATABASE_MAX_CONNECTIONS`. Notifications are bounded,
owner-scoped hints emitted transactionally after commit; duplicates or losses
remain safe because REST pull is authoritative.

Supabase Postgres is the source of truth. IndexedDB accepts optimistic local
writes and keeps an idempotent outbox, REST performs push/pull reconciliation,
and HTMX-owned WebSocket messages only trigger a durable REST pull. The client
opens its fallback socket only when embedded outside the Maud application shell.

## Health checks

- Liveness: `GET /healthz` -> `200 OK` without dependency probes.
- Readiness: `GET /readyz` verifies database availability.
- App/API status: `GET /api/v1/health` and `GET /api/v1/info` (with legacy
  `/api/health` and `/api/info` aliases).
- WebSocket: authenticated `GET /ws` upgrades only after auth and, for browser
  sessions, an exact allowed-Origin check.

## Pinning for a release

```sh
scripts/pin-submodules.sh main
git commit -m "Pin canonical apps to main"
```

The committed superproject SHA is the deployable release: it references exact
app commits, so a checkout + `./build.sh` reproduces the shipped stack.
