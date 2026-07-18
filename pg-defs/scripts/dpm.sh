#!/usr/bin/env bash
# Declarative Postgres migration for pg-defs, via dpm
# (https://github.com/declarative-migrations/declarative-postgres-migrate.rs).
#
# schema/schema.sql is the source of truth; the target database converges onto
# it. dpm materializes schema.sql on a shadow server, introspects both sides
# from pg_catalog, and emits ordered, reviewable SQL. It replaces the
# hand-rolled differ in src/diff.mjs as the migration generator (diff.mjs
# remains available as an independent second opinion).
#
# Usage:
#   scripts/dpm.sh diff        # print the migration SQL (default; never executes)
#   scripts/dpm.sh verify      # rehearse on a shadow replica, prove convergence
#   scripts/dpm.sh review      # diff + AI review of the migration
#   scripts/dpm.sh apply       # generate + execute (interactive confirm)
#   scripts/dpm.sh bootstrap   # full DDL for an empty database
# Extra arguments pass through to dpm (e.g. --fail-on-diff, --out FILE,
# --cross-check-all). See `dpm help`.
#
# Env:
#   TARGET_DATABASE_URL   database to converge; falls back (in order) to
#                         AGENT_TASKS_RDS_DATABASE_URL, RDS_DATABASE_URL,
#                         DATABASE_URL, PG_DATABASE_URL — the same resolution
#                         order as src/diff.mjs.
#   SHADOW_DATABASE_URL   a server where dpm may CREATE/DROP throwaway
#                         databases (schema.sql sources are materialized
#                         there). Never point this at production.
#
# Safety: destructive statements are emitted commented-out, and `apply`
# refuses to execute live destructive SQL, unless the two dpm consent flags
# (--allow-destructive-sql / --allow-destructive-ops) are passed explicitly.
# Never apply migrations automatically; a human reviews the SQL first.
set -euo pipefail

cmd="${1:-diff}"
[ "$#" -gt 0 ] && shift

pg_defs_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
schema_sql="$pg_defs_dir/schema/schema.sql"

if ! command -v dpm >/dev/null 2>&1; then
  echo "error: dpm not found on PATH." >&2
  echo "install: brew install declarative-migrations/tap/dpm" >&2
  echo "     or (pin the ref before piping to bash):" >&2
  echo "         curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/declarative-migrations/declarative-postgres-migrate.rs/f7180770fc0c7a3dbf9b83dcdc2ac6255da31ffc/scripts/install.sh | bash" >&2
  exit 1
fi

if [ -z "${SHADOW_DATABASE_URL:-}" ]; then
  echo "error: SHADOW_DATABASE_URL is required — a Postgres server URL where dpm" >&2
  echo "may create/drop throwaway databases to materialize schema.sql." >&2
  echo "Local example: postgres://postgres:postgres@localhost:5432/postgres" >&2
  exit 1
fi

target="${TARGET_DATABASE_URL:-${AGENT_TASKS_RDS_DATABASE_URL:-${RDS_DATABASE_URL:-${DATABASE_URL:-${PG_DATABASE_URL:-}}}}}"

case "$cmd" in
  bootstrap)
    exec dpm bootstrap --source "$schema_sql" "$@"
    ;;
  diff | verify | apply | review)
    if [ -z "$target" ]; then
      echo "error: no target database URL. Set TARGET_DATABASE_URL (or one of" >&2
      echo "AGENT_TASKS_RDS_DATABASE_URL, RDS_DATABASE_URL, DATABASE_URL, PG_DATABASE_URL)." >&2
      exit 1
    fi
    # Hand the target (which carries the password) to dpm via the environment,
    # not `--target` on its argv — argv is world-readable via `ps`/procfs, and
    # SHADOW_DATABASE_URL already reaches dpm the same way. dpm reads
    # TARGET_DATABASE_URL directly, so the credential never appears in a process
    # listing.
    export TARGET_DATABASE_URL="$target"
    exec dpm "$cmd" --source "$schema_sql" "$@"
    ;;
  *)
    exec dpm "$cmd" "$@"
    ;;
esac
