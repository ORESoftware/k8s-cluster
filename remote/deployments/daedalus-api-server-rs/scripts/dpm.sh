#!/usr/bin/env bash
# Declarative Postgres migration for the daedalus schema, via dpm
# (https://github.com/declarative-migrations/declarative-postgres-migrate.rs).
#
# NOTE: the source of truth is the SHARED pg-defs contract at
# remote/libs/pg-defs/schema/schema.sql, not a file in this repo. The daedalus
# tables are a named schema (`daedalus`) inside that shared database, following
# the fiducia/benefactor pattern — so a diff generated here covers the whole
# shared contract, not just daedalus. Review the emitted SQL accordingly.
#
# Usage:
#   scripts/dpm.sh diff        # print the migration SQL (default; never executes)
#   scripts/dpm.sh verify      # rehearse on a shadow replica, prove convergence
#   scripts/dpm.sh review      # diff + AI review of the migration
#   scripts/dpm.sh apply       # generate + execute (interactive confirm)
#   scripts/dpm.sh bootstrap   # full DDL for an empty database
# Extra arguments pass through to dpm (e.g. --fail-on-diff, --out FILE).
# See `dpm help`.
#
# Env:
#   TARGET_DATABASE_URL   database to converge; falls back (in order) to
#                         DAEDALUS_API_DATABASE_URL, DATABASE_URL,
#                         RDS_DATABASE_URL — the same resolution order as
#                         src/persistence.rs, so the server and this script can
#                         never disagree about which database they mean.
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

service_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
schema_sql="$service_dir/../../libs/pg-defs/schema/schema.sql"

if [ ! -f "$schema_sql" ]; then
  echo "error: shared pg-defs schema not found at $schema_sql" >&2
  echo "This script only works with the repo checked out at" >&2
  echo "remote/deployments/ inside the k8s-cluster superproject." >&2
  exit 1
fi

if ! command -v dpm >/dev/null 2>&1; then
  echo "error: dpm not found on PATH." >&2
  echo "install: brew install declarative-migrations/tap/dpm" >&2
  exit 1
fi

if [ -z "${SHADOW_DATABASE_URL:-}" ]; then
  echo "error: SHADOW_DATABASE_URL is required — a Postgres server URL where dpm" >&2
  echo "may create/drop throwaway databases to materialize schema.sql." >&2
  echo "Local example: postgres://postgres:postgres@localhost:5432/postgres" >&2
  exit 1
fi

target="${TARGET_DATABASE_URL:-${DAEDALUS_API_DATABASE_URL:-${DATABASE_URL:-${RDS_DATABASE_URL:-}}}}"

case "$cmd" in
  bootstrap)
    exec dpm bootstrap --source "$schema_sql" "$@"
    ;;
  diff | verify | apply | review)
    if [ -z "$target" ]; then
      echo "error: no target database URL. Set TARGET_DATABASE_URL (or one of" >&2
      echo "DAEDALUS_API_DATABASE_URL, DATABASE_URL, RDS_DATABASE_URL)." >&2
      exit 1
    fi
    # Hand the target (which carries the password) to dpm via the environment,
    # not `--target` on its argv — argv is world-readable via `ps`/procfs.
    export TARGET_DATABASE_URL="$target"
    exec dpm "$cmd" --source "$schema_sql" "$@"
    ;;
  *)
    exec dpm "$cmd" "$@"
    ;;
esac
