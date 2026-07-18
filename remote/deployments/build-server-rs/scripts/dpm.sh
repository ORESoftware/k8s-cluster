#!/usr/bin/env bash
# Declarative Postgres migration for dd-build-server, via dpm
# (https://github.com/declarative-migrations/declarative-postgres-migrate.rs).
#
# The schema contract lives with the shared defs:
#   remote/libs/pg-defs/schema/databases/dd_build_server/schema.sql
# It targets the build server's OWN database (conventionally `dd_build_server`
# on the shared Amazon RDS instance) — its own namespace, separate from the
# shared pg-defs contract database. The target converges onto the contract;
# the server itself never migrates at boot.
#
# Usage:
#   scripts/dpm.sh diff        # print the migration SQL (default; never executes)
#   scripts/dpm.sh verify      # rehearse on a shadow replica, prove convergence
#   scripts/dpm.sh review      # diff + AI review of the migration
#   scripts/dpm.sh apply       # generate + execute (interactive confirm)
#   scripts/dpm.sh bootstrap   # full DDL for an empty database
# Extra arguments pass through to dpm (e.g. --fail-on-diff, --out FILE).
#
# Env:
#   TARGET_DATABASE_URL   database to converge; falls back (in order) to
#                         BUILD_SERVER_DATABASE_URL, DATABASE_URL — the same
#                         resolution order as the server config.
#   SHADOW_DATABASE_URL   a server where dpm may CREATE/DROP throwaway
#                         databases (the schema contract is materialized
#                         there). Never point this at production.
#
# Safety: destructive statements are emitted commented-out, and `apply`
# refuses to execute live destructive SQL, unless the two dpm consent flags
# (--allow-destructive-sql / --allow-destructive-ops) are passed explicitly.
# Never apply migrations automatically; a human reviews the SQL first.
set -euo pipefail

cmd="${1:-diff}"
[ "$#" -gt 0 ] && shift

service_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
schema_sql="$service_dir/../../libs/pg-defs/schema/databases/dd_build_server/schema.sql"

if [ ! -f "$schema_sql" ]; then
  echo "error: schema contract not found at $schema_sql" >&2
  echo "(is the remote/libs submodule initialized?)" >&2
  exit 1
fi

if ! command -v dpm >/dev/null 2>&1; then
  echo "error: dpm not found on PATH." >&2
  echo "install: brew install declarative-migrations/tap/dpm" >&2
  echo "     or (pin the ref before piping to bash):" >&2
  echo "         curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/declarative-migrations/declarative-postgres-migrate.rs/f7180770fc0c7a3dbf9b83dcdc2ac6255da31ffc/scripts/install.sh | bash" >&2
  exit 1
fi

if [ -z "${SHADOW_DATABASE_URL:-}" ]; then
  echo "error: SHADOW_DATABASE_URL is required — a Postgres server URL where dpm" >&2
  echo "may create/drop throwaway databases to materialize the schema contract." >&2
  echo "Local example: postgres://postgres:postgres@localhost:5432/postgres" >&2
  exit 1
fi

target="${TARGET_DATABASE_URL:-${BUILD_SERVER_DATABASE_URL:-${DATABASE_URL:-}}}"

case "$cmd" in
  bootstrap)
    exec dpm bootstrap --source "$schema_sql" "$@"
    ;;
  diff | verify | apply | review)
    if [ -z "$target" ]; then
      echo "error: no target database URL. Set TARGET_DATABASE_URL (or one of" >&2
      echo "BUILD_SERVER_DATABASE_URL, DATABASE_URL)." >&2
      exit 1
    fi
    exec dpm "$cmd" --source "$schema_sql" --target "$target" "$@"
    ;;
  *)
    exec dpm "$cmd" "$@"
    ;;
esac
