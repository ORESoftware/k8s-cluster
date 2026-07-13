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
`canonical-web-server.rs` is the dynamic sMASH application: Supabase Auth and
Postgres, Maud, Axum, SeaORM, and HTMX. It serves server-rendered application
pages, a versioned REST API, and authenticated WebSockets. Its TypeScript client
uses IndexedDB for optimistic/offline state and reconciles with authoritative
Supabase Postgres through the REST API. The Maud shell gives HTMX ownership of
the dashboard WebSocket, while PostgreSQL `LISTEN`/`NOTIFY` relays disposable,
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
./build.sh            # builds Astro, the HTMX/IndexedDB client, and the locked
                      # Rust application server in their own submodules
```

To run the result, copy `.env.example` to an ignored `.env.local`, replace all
placeholders, and load it into the environment. Apply migrations with the
privileged migration-only URL, remove that URL from the environment, then start
the long-lived process with only the least-privilege runtime URL:

```sh
set -a; source .env.local; set +a
./apps/canonical-web-server.rs/target/release/canonical-web-server migrate
unset MIGRATION_DATABASE_URL MIGRATION_DATABASE_MAX_CONNECTIONS
./apps/canonical-web-server.rs/target/release/canonical-web-server serve
```

After the first migration, apply the explicit runtime-role grants in
`apps/canonical-web-server.rs/deploy/postgres/bootstrap_runtime_role.sql`.
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
build.sh               # Astro + app client + locked Rust application build
```
