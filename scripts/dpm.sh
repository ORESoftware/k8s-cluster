#!/usr/bin/env bash
# Declarative Postgres migration for billing-server-rs, via dpm
# (https://github.com/declarative-migrations/declarative-postgres-migrate.rs).
#
# schema/schema.sql plus schema/fragments/*.sql are the source of truth. The
# fragments keep security-sensitive additions reviewable without rewriting the
# large historical schema file; this script deterministically concatenates the
# base followed by lexicographically ordered fragments before invoking dpm.
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
#                         BILLING_DATABASE_URL, DATABASE_URL — the same
#                         resolution order as src/config.rs. This is the
#                         service's OWN database, separate from the shared
#                         pg-defs RDS contract.
#   SHADOW_DATABASE_URL   a server where dpm may CREATE/DROP throwaway
#                         databases. Never point this at production.
#
# Safety: destructive statements are emitted commented-out, and `apply`
# refuses to execute live destructive SQL unless the two dpm consent flags are
# passed explicitly. Never apply migrations automatically; a human reviews the
# generated SQL first.
set -euo pipefail

cmd="${1:-diff}"
[ "$#" -gt 0 ] && shift

billing_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
base_schema="$billing_dir/schema/schema.sql"
fragment_dir="$billing_dir/schema/fragments"
combined_schema="$(mktemp "${TMPDIR:-/tmp}/quaestor-schema.XXXXXX.sql")"
trap 'rm -f "$combined_schema"' EXIT

if [ ! -s "$base_schema" ]; then
  echo "error: missing or empty base schema: $base_schema" >&2
  exit 1
fi

{
  cat "$base_schema"
  printf '\n\n-- BEGIN DECLARATIVE SCHEMA FRAGMENTS --\n'
  if [ -d "$fragment_dir" ]; then
    while IFS= read -r -d '' fragment; do
      printf '\n-- BEGIN %s --\n' "${fragment#$billing_dir/}"
      cat "$fragment"
      printf '\n-- END %s --\n' "${fragment#$billing_dir/}"
    done < <(find "$fragment_dir" -maxdepth 1 -type f -name '*.sql' -print0 | sort -z)
  fi
} > "$combined_schema"

if ! command -v dpm >/dev/null 2>&1; then
  echo "error: dpm not found on PATH." >&2
  echo "install: brew install declarative-migrations/tap/dpm" >&2
  echo "     or: curl -fsSL https://raw.githubusercontent.com/declarative-migrations/declarative-postgres-migrate.rs/main/scripts/install.sh | bash" >&2
  exit 1
fi

if [ -z "${SHADOW_DATABASE_URL:-}" ]; then
  echo "error: SHADOW_DATABASE_URL is required — a Postgres server URL where dpm" >&2
  echo "may create/drop throwaway databases to materialize the composed schema." >&2
  echo "Local example: postgres://postgres:postgres@localhost:5432/postgres" >&2
  exit 1
fi

target="${TARGET_DATABASE_URL:-${BILLING_DATABASE_URL:-${DATABASE_URL:-}}}"

case "$cmd" in
  bootstrap)
    dpm bootstrap --source "$combined_schema" "$@"
    ;;
  diff | verify | apply | review)
    if [ -z "$target" ]; then
      echo "error: no target database URL. Set TARGET_DATABASE_URL (or one of" >&2
      echo "BILLING_DATABASE_URL, DATABASE_URL)." >&2
      exit 1
    fi
    dpm "$cmd" --source "$combined_schema" --target "$target" "$@"
    ;;
  *)
    dpm "$cmd" "$@"
    ;;
esac
