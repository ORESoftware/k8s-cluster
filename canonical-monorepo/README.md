# canonical-monorepo

Git superproject for the [canonical-cloud](https://github.com/canonical-cloud)
repositories.

Each application/service repo is tracked as a git **submodule** under `apps/`.
The superproject pins each submodule to an exact commit, while `.gitmodules`
sets `branch = main` for every submodule so updates intentionally follow each
repo's main branch.

## Apps

| Submodule                            | Stack                      | Repo |
| ------------------------------------ | -------------------------- | ---- |
| `apps/canonical-web-server.rs`       | sMASH + TypeScript/IndexedDB | [canonical-web-server.rs](https://github.com/canonical-cloud/canonical-web-server.rs) |
| `apps/canonical-marketing-site.web` | Astro                      | [canonical-marketing-site.web](https://github.com/canonical-cloud/canonical-marketing-site.web) |
| `apps/canonical-interfaces`          | JSON Schema / SQL          | [canonical-interfaces](https://github.com/canonical-cloud/canonical-interfaces) |

`canonical-marketing-site.web` is the static public site.
`canonical-web-server.rs` is a modular Rust workspace. Its customer-facing
sMASH binary uses Supabase Auth/Postgres, Maud, Axum, SeaORM, and HTMX to serve
server-rendered application pages, a versioned REST API, and authenticated
WebSockets. A separate no-ingress `canonical-session-revoker` binary retries
durable upstream logout work with its own least-privilege database identity.
Shared auth, configuration, session, and persistence code lives in focused
workspace crates rather than inside either process. The TypeScript client uses
IndexedDB for optimistic/offline state and reconciles with authoritative
Supabase Postgres through REST. The Maud shell gives HTMX ownership of the
dashboard WebSocket, while PostgreSQL `LISTEN`/`NOTIFY` relays disposable,
owner-scoped invalidation hints between server instances. `canonical-interfaces`
remains the typed-IO source of truth. See `docs/repo-boundaries.md`.

## Clone

```sh
git clone --recurse-submodules git@github.com:canonical-cloud/canonical-monorepo.git
```

For an existing checkout:

```sh
git submodule update --init --recursive
```

## Build the full stack

```sh
./build.sh            # builds Astro, the HTMX/IndexedDB client, and all locked
                      # Rust workspace binaries in their own submodules
```

To run the result, derive three ignored environment files from `.env.example`:
one for the migration job, one for the customer web process, and one for the
no-ingress revoker. Never load all three database credentials into one process.
Apply migrations and bootstrap both runtime roles with the privileged
migration-only URL:

```sh
set -a; source .env.migration; set +a
./apps/canonical-web-server.rs/target/release/canonical-web-server migrate
psql "$MIGRATION_DATABASE_URL" \
  --file apps/canonical-web-server.rs/deploy/postgres/bootstrap_runtime_role.sql
psql "$MIGRATION_DATABASE_URL" \
  --file apps/canonical-web-server.rs/deploy/postgres/bootstrap_session_revoker_role.sql
unset MIGRATION_DATABASE_URL MIGRATION_DATABASE_MAX_CONNECTIONS
```

Then launch the long-lived processes independently. The web environment has
only `DATABASE_URL`; the worker environment has only
`SESSION_REVOCATION_DATABASE_URL` and receives no network ingress:

```sh
set -a; source .env.web; set +a
./apps/canonical-web-server.rs/target/release/canonical-web-server serve

# Separate shell/container/process:
set -a; source .env.revoker; set +a
./apps/canonical-web-server.rs/target/release/canonical-session-revoker run
```

Production requires Supabase session/direct pooling; transaction pooling cannot
support the dedicated PostgreSQL listener connection.

## Update pins

```sh
scripts/pin-submodules.sh main
git status
git diff --cached --submodule
git commit -m "Pin canonical apps to main"
```

The script verifies the target branch exists on every submodule remote, refuses
dirty submodule checkouts, updates every `.gitmodules` `branch` entry,
fast-forwards each submodule, and stages the resulting gitlink pins. Preview
with `--dry-run`.

After the full pinned-stack CI succeeds on `main`, the release workflow
publishes separately attested web and no-ingress revoker images to GHCR, tagged
with the exact monorepo commit. Deployment state and digest promotion live in
`ORESoftware/k8s-cluster`; Argo CD, not GitHub Actions, reconciles the backend.
See `docs/deploy.md` for the credential and migration boundaries.

## Feature branches

Switch the superproject and every app submodule to the same feature branch:

```sh
scripts/checkout-feature-branch.sh feature/new-landing
```

## Audit

```sh
scripts/audit-repo-state.sh          # conflict markers, stray secrets, submodule wiring
```

## Layout

```text
apps/                  # git submodules (the app repos)
.nix/ .envrc shell     # nix dev shell
scripts/               # pin / checkout / audit helpers (no destructive git)
tests/                 # node --test superproject contract specs
docs/                  # repo-boundaries, deploy
.github/               # CI + dependabot
build.sh               # Astro + app client + locked Rust workspace build
```
