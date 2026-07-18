#!/usr/bin/env bash
# Declarative Postgres migration for the dd-embeddings-rs search index, via dpm
# (https://github.com/declarative-migrations/declarative-postgres-migrate.rs).
#
# schema/schema.sql is the source of truth; the target database converges onto
# it. dpm materializes schema.sql on a shadow server, introspects both sides
# from pg_catalog, and emits ordered, reviewable SQL. It replaces the frozen
# boot-time sqlx migration under migrations/ (kept as a historical record
# only) as the migration workflow — the server itself never migrates at boot.
# This mirrors billing-server-rs and remote/libs/pg-defs/scripts/dpm.sh.
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
#                         EMBEDDINGS_DATABASE_URL, DATABASE_URL,
#                         RDS_DATABASE_URL — the same resolution order as
#                         src/config.rs. This is the search subsystem's OWN
#                         database, separate from the shared pg-defs RDS
#                         contract.
#   SHADOW_DATABASE_URL   a server where dpm may CREATE/DROP throwaway
#                         databases (schema.sql sources are materialized
#                         there). It must have the `vector` (pgvector >= 0.5)
#                         and `pg_trgm` extensions installed. Never point this
#                         at production.
#
# Safety: destructive statements are emitted commented-out, and `apply`
# refuses to execute live destructive SQL, unless the two dpm consent flags
# (--allow-destructive-sql / --allow-destructive-ops) are passed explicitly.
# Never apply migrations automatically; a human reviews the SQL first.
set -euo pipefail

cmd="${1:-diff}"
[ "$#" -gt 0 ] && shift

service_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
schema_sql="$service_dir/schema/schema.sql"

if ! command -v dpm >/dev/null 2>&1; then
  echo "error: dpm not found on PATH." >&2
  echo "install: brew install declarative-migrations/tap/dpm" >&2
  echo "     or: curl -fsSL https://raw.githubusercontent.com/declarative-migrations/declarative-postgres-migrate.rs/main/scripts/install.sh | bash" >&2
  exit 1
fi

if [ -z "${SHADOW_DATABASE_URL:-}" ]; then
  echo "error: SHADOW_DATABASE_URL is required — a Postgres server URL where dpm" >&2
  echo "may create/drop throwaway databases to materialize schema.sql." >&2
  echo "Local example: postgres://postgres:postgres@localhost:5432/postgres" >&2
  exit 1
fi

target="${TARGET_DATABASE_URL:-${EMBEDDINGS_DATABASE_URL:-${DATABASE_URL:-${RDS_DATABASE_URL:-}}}}"

case "$cmd" in
  bootstrap)
    exec dpm bootstrap --source "$schema_sql" "$@"
    ;;
  diff | verify | apply | review)
    if [ -z "$target" ]; then
      echo "error: no target database URL. Set TARGET_DATABASE_URL (or one of" >&2
      echo "EMBEDDINGS_DATABASE_URL, DATABASE_URL, RDS_DATABASE_URL)." >&2
      exit 1
    fi
    exec dpm "$cmd" --source "$schema_sql" --target "$target" "$@"
    ;;
  *)
    exec dpm "$cmd" "$@"
    ;;
esac
