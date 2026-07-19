# Deploy

The canonical.cloud stack has a static Astro marketing site, a customer-facing
sMASH application server, and a no-ingress session-revocation worker. The web
binary serves Maud/HTMX pages, versioned REST, authenticated WebSockets, and the
built TypeScript/IndexedDB client. It can also serve the prebuilt marketing site
as its final static fallback. The Maud shell declares the HTMX WebSocket
extension on `/ws`; typed socket frames wake the REST pull loop instead of
directly mutating durable client state. The worker is a separate binary and
process; it never serves HTTP and cannot use the customer database credential.

## Build all assets

```sh
git submodule update --init --recursive
./build.sh                      # Astro + app client + locked Rust workspace bins
```

The script builds `canonical-marketing-site.web/dist`, verifies and builds
`canonical-web-server.rs/client/dist`, and builds every locked Rust workspace
binary, including `canonical-web-server` and `canonical-session-revoker`. Each
artifact stays in its owning submodule; the root `.env.example` points
`STATIC_DIR` and `APP_ASSET_DIR` at the browser outputs.

## Process and credential boundaries

| Process | Command | Database identity | Ingress |
| --- | --- | --- | --- |
| Migration job | `canonical-web-server migrate` | migration/table owner via `MIGRATION_DATABASE_URL` | none; one shot |
| Customer web | `canonical-web-server serve` | `canonical_web_server` via `DATABASE_URL` | HTTPS/WebSocket on port 8081 |
| Session revoker | `canonical-session-revoker run` | `canonical_session_revoker` via `SESSION_REVOCATION_DATABASE_URL` | none |

Deploy these as separate process specifications with separate secret mounts.
Sharing a container image layer does not justify sharing an environment,
service account, network policy, or database credential.

## 1. Customer application-server process

Build the `web` target in `apps/canonical-web-server.rs/Dockerfile`. The image
contains only the web binary and authenticated client, and runs as a distroless
nonroot user. Supply the marketing build at `/app/static` as a read-only volume
directly from the marketing submodule, or add it in a higher-level release
image. The `.env.web` secret set must not contain the migration-owner URL or
the revoker's separate database URL.

```sh
docker build --target web -t canonical-web-server apps/canonical-web-server.rs
docker run --env-file .env.web \
  -e STATIC_DIR=/app/static -e APP_ASSET_DIR=/app/client \
  -p 8081:8081 \
  -v "$PWD/apps/canonical-marketing-site.web/dist:/app/static:ro" \
  canonical-web-server serve
```

## 2. No-ingress session-revocation process

Build the `revoker` target from the same reviewed source. This image contains
only `canonical-session-revoker`: it has no browser assets, HTTP listener, or
exposed port. Give it outbound TLS access to Supabase Auth and Postgres, but no
inbound service or route. Its `.env.revoker` contains the same session
encryption key and Supabase publishable key as the web process, plus only the
revoker-scoped database URL.

```sh
docker build --target revoker -t canonical-session-revoker apps/canonical-web-server.rs
docker run --env-file .env.revoker canonical-session-revoker run
```

The worker performs an exact database-role check before its loop starts, so a
customer or owner identity fails closed. Its idempotent bootstrap separately
rejects role memberships and reasserts `NOBYPASSRLS` before deployment.

## 3. Split marketing site and application server

Serve the static site from the marketing site's nginx image
(`apps/canonical-marketing-site.web/Dockerfile`) and run the web server for
`/login`, `/auth/*`, `/app/*`, `/app-assets/*`, `/api/*`, `/ws`, `/healthz`, and
`/readyz`. The edge must preserve cookies and `Origin`, forward WebSocket
upgrade headers on `/ws`, and use timeouts suitable for long-lived connections.
Point the marketing-site build's base at the public path with `PUBLIC_BASE`.

## Required runtime configuration

Start from the root `.env.example`, split its variables by the process table
above, and inject real values with the deployment platform rather than
committing them:

- the web process gets `DATABASE_URL` for the dedicated least-privilege
  `canonical_web_server` role, which neither owns tables nor has `BYPASSRLS`;
- the no-ingress worker gets `SESSION_REVOCATION_DATABASE_URL` for the distinct
  `canonical_session_revoker` role and a small
  `SESSION_REVOCATION_DATABASE_MAX_CONNECTIONS` pool; this URL is absent from
  the web process;
- `MIGRATION_DATABASE_URL` and `MIGRATION_DATABASE_MAX_CONNECTIONS` only in the
  one-shot migration job, never in either long-lived environment;
- `SUPABASE_URL` and a Supabase publishable key (never a secret/service-role
  key) for both Auth callers;
- a unique 32-byte `APP_SESSION_ENCRYPTION_KEY`, base64 encoded, plus the exact
  public `APP_BASE_URL` and `APP_ALLOWED_ORIGINS` in the web process; the worker
  receives the same encryption key but no public-origin or cookie settings;
- `COOKIE_SECURE=true`, a `__Host-` session cookie, bounded
  `LOGIN_RATE_LIMIT_*` settings, `LOGIN_AUTH_MAX_CONCURRENCY`, and
  `BEARER_AUTH_MAX_CONCURRENCY`. The
  application performs bounded per-account and global per-process login
  throttles and caps concurrent Supabase bearer verification calls; the
  gateway must enforce the authoritative trusted-client-IP limit before
  requests reach the server; and
- `STATIC_DIR` and `APP_ASSET_DIR` pointing at the two built browser asset
  directories.

Set `OTEL_EXPORTER_OTLP_ENDPOINT` to the cluster collector's OTLP/gRPC endpoint
to export explicit HTTP traces and low-cardinality request metrics. The
collector exposes OTLP metrics to Prometheus. Structured application logs stay
on stdout for Kubernetes CRI collection by Promtail and Loki; they are not sent
through the OTLP exporter.

The public gateway must terminate TLS, redirect cleartext traffic, and set HSTS
for the public hostname. It must preserve `Origin` and WebSocket upgrade
headers only for the application routes. The Rust fallback and the marketing
nginx image both send CSP, anti-framing, referrer, permissions, and opener
headers; do not remove or overwrite them at the gateway.

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

Run SeaORM migrations and establish both explicit long-lived process roles
before serving:

```sh
set -a; source .env.migration; set +a
./apps/canonical-web-server.rs/target/release/canonical-web-server migrate
psql "$MIGRATION_DATABASE_URL" \
  --file apps/canonical-web-server.rs/deploy/postgres/bootstrap_runtime_role.sql
psql "$MIGRATION_DATABASE_URL" \
  --file apps/canonical-web-server.rs/deploy/postgres/bootstrap_session_revoker_role.sql
unset MIGRATION_DATABASE_URL MIGRATION_DATABASE_MAX_CONNECTIONS
```

The bootstraps create/reassert `canonical_web_server` and
`canonical_session_revoker` as non-owner, non-`BYPASSRLS`, membership-free
logins. The customer role gets only its explicit application allow-list; the
revoker gets only the `web_session` operations required inside its fixed,
transaction-local revocation task. Configure passwords outside Git, keep each
URL in its own process environment, and re-run both scripts when migrations
change their allow-lists. The `serve` command has no automatic migration path
and never receives owner credentials.

`bootstrap_admin_role.sql` is intentionally not part of this release startup.
It exists for a future separately deployed admin application, which must first
define its own origin, binary, MFA/reauthentication flow, database credential,
secret-manager policy, and immutable audit path.

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

Local logout is authoritative immediately. The separately deployed
`canonical-session-revoker run` process retries failed upstream Supabase
sign-out with a database-backed lease and bounded exponential backoff; it also
revokes expired local sessions and prunes only terminal revocations after the
retention window. Its process-specific RLS policy requires the exact worker
login and fixed transaction-local task marker. This worker is not a general
system-job identity and must not be extended to bypass customer RLS. Future
administrative or background services need their own least-privilege identity.

Keep privileged operations out of the customer application. Before an admin
surface exists, define its separate origin/service, MFA or reauthentication,
immutable audit records, and narrowly scoped Supabase credential. Never place a
service-role key in this server's runtime environment.

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

## CI, image delivery, and Argo CD

`ci.yml` proves the complete pinned stack before `release.yml` is allowed to
publish anything. After a successful `main` push, the release workflow checks
out the tested monorepo SHA, refuses to run if that SHA is no longer current
`main`, and publishes two separate GHCR images:

- `ghcr.io/canonical-cloud/canonical-web-server:<monorepo-sha>`;
- `ghcr.io/canonical-cloud/canonical-session-revoker:<monorepo-sha>`.

Both images include an SBOM, BuildKit provenance, and a GitHub artifact
attestation. The workflow publishes no floating tags. GitOps must pin the
release SHA or, preferably, the exact image digests printed in the release run
summary.

GitHub Actions never receives a kubeconfig and never runs `kubectl`. Backend
runtime state lives in `ORESoftware/k8s-cluster`, where a dedicated
canonical-cloud Argo CD Application reconciles reviewed digest changes. A
rollback is a Git revert to the previous digests. Database migrations remain a
separately reviewed operator job using the migration-only credential; they are
never an Argo sync hook or an automatic release step.

The release summary prints the exact command for the cluster repository's
`remote/argocd/canonical-cloud/promote-release.mjs` helper. That helper updates
only the two image digests and their shared monorepo release annotation; the
result is reviewed, tested, and committed to `k8s-cluster@dev` before Argo sees
it.
