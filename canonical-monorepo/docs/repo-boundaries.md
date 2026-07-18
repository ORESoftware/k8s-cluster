# Repo boundaries

`canonical-monorepo` is a **git superproject**: it stores submodule *pins*
(gitlinks) plus shared config (CI, docs, scripts, `build.sh`). It never vendors
app source directly.

## Apps

- **`canonical-web-server.rs`** — modular Rust workspace. Its root package is
  the customer-facing sMASH application server (Supabase Auth/Postgres, Maud
  HTML, Axum REST/WebSockets, SeaORM, and HTMX); it owns login/session handling,
  server-rendered `/app` pages, `/api/v1/*`, `/ws`, and the TypeScript IndexedDB
  sync client. Focused crates under `crates/` provide auth, configuration,
  session/revocation, and persistence boundaries. The separately deployable
  `services/canonical-session-revoker` process has no ingress and reconciles
  durable logout state using its own database role. Supabase Postgres is
  authoritative; IndexedDB is an optimistic/offline cache with a durable outbox.
  The Maud shell lets HTMX own the dashboard socket; a PostgreSQL
  `LISTEN`/`NOTIFY` backplane relays owner-scoped wake-ups across instances.
  Database migrations and process-specific role bootstrap SQL also live here.
  Public.
- **`canonical-marketing-site.web`** — Astro static marketing site (SOC 2 /
  FedRAMP / HIPAA). Builds to `dist/`, which the web server serves. Public.
- **`canonical-interfaces`** — typed-IO source of truth: JSON Schema for the
  HTTP API + SQL for the compliance store, generated into TS/Rust/Python/Go
  adapters. The web server and clients consume its generated types. Public.

Each app is its own repo with its own visibility, CI, Dockerfile, `agents.md`,
and Nix dev shell. The superproject is the all-up integration / GitOps view.

## Where things live

| Concern                                  | Home                                             |
| ---------------------------------------- | ------------------------------------------------ |
| App source                               | the app repo (`apps/<app>`, a submodule)         |
| Static public marketing pages            | `canonical-marketing-site.web`                   |
| Customer HTML, REST, WebSockets, sync     | `canonical-web-server.rs` root package           |
| Shared auth/session/config/store modules  | `canonical-web-server.rs/crates`                 |
| Durable logout reconciliation             | `canonical-web-server.rs/services/canonical-session-revoker` |
| Migrations and process-role grants         | `canonical-web-server.rs/deploy/postgres`        |
| Cross-repo asset and application build   | `build.sh` here                                  |
| Submodule pins / branch pins             | `.gitmodules` + gitlinks here                    |
| Shared automation                        | `scripts/` here                                  |

## Rules

- Change app code inside the submodule, push it there, **then** update the pin
  here with `scripts/pin-submodules.sh`.
- Do not commit real `.env*` files. Only `.env.example` (placeholder values) is
  tracked; everything else matching `.env*` is gitignored.
- Browsers never receive database credentials or server-held Supabase tokens.
  The REST sync protocol is the durable path; WebSockets are authenticated
  invalidation hints, and IndexedDB never becomes the source of truth.
- The privileged `MIGRATION_DATABASE_URL` belongs only in the one-shot
  `canonical-web-server migrate` job. The long-lived `serve` process receives
  only the non-owner, non-`BYPASSRLS` `DATABASE_URL`; its PostgreSQL backplane
  needs session/direct pooling and one connection beyond the SeaORM pool.
- The no-ingress `canonical-session-revoker` receives only
  `SESSION_REVOCATION_DATABASE_URL`, which must authenticate as the exact
  non-owner, non-`BYPASSRLS` `canonical_session_revoker` role. The web process
  must never receive that credential, and the worker must never receive
  `DATABASE_URL` or the migration owner URL.
- Administrative capabilities stay outside both deployed processes. A future
  admin application requires a separate origin, binary, database identity,
  MFA-backed actor context, secret-manager scope, and immutable audit path.
- Keep destructive actions manual: the `scripts/*.sh` helpers never `git push`
  and guard with `--dry-run` / `--allow-dirty`.
- Removing a submodule checkout with `rm -rf apps/<app>` corrupts the gitlink
  state — use `git submodule` commands (or `git rm` for a real removal) instead.
