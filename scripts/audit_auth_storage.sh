#!/usr/bin/env bash
# Run read-only security audits against the production Supabase and/or RDS
# databases. The script never accepts API keys; it needs short-lived/operator
# Postgres DSNs with catalog visibility.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

command -v psql >/dev/null || { echo "psql is required" >&2; exit 2; }

export PGCONNECT_TIMEOUT="${PGCONNECT_TIMEOUT:-8}"
export PGSSLMODE="${PGSSLMODE:-require}"
case "$PGSSLMODE" in
  require|verify-ca|verify-full) ;;
  *) echo "PGSSLMODE must be require, verify-ca, or verify-full" >&2; exit 2 ;;
esac

warnings_as_errors="${SONUS_AUDIT_WARNINGS_AS_ERRORS:-0}"
[[ "$warnings_as_errors" == 0 || "$warnings_as_errors" == 1 ]] || {
  echo "SONUS_AUDIT_WARNINGS_AS_ERRORS must be 0 or 1" >&2
  exit 2
}

reject_weak_ssl_dsn() {
  local dsn="$1"
  if [[ "$dsn" =~ sslmode=(disable|allow|prefer) ]]; then
    echo "database DSN explicitly weakens TLS via sslmode=${BASH_REMATCH[1]}" >&2
    exit 2
  fi
}

run_audit() {
  local label="$1"
  local dsn="$2"
  local sql_file="$3"
  local output

  reject_weak_ssl_dsn "$dsn"
  echo "Auditing $label ..."
  output="$(
    psql "$dsn" \
      --no-psqlrc \
      --set=ON_ERROR_STOP=1 \
      --tuples-only \
      --no-align \
      --field-separator='|' \
      --file="$sql_file"
  )"

  if [[ -z "${output//[[:space:]]/}" ]]; then
    echo "$label: clean"
    return 0
  fi

  printf '%s\n' "$output" >&2
  if grep -q '^critical|' <<<"$output"; then
    echo "$label: critical security violations found" >&2
    return 1
  fi
  if [[ "$warnings_as_errors" == 1 ]] && grep -q '^warning|' <<<"$output"; then
    echo "$label: warnings treated as errors" >&2
    return 1
  fi
  echo "$label: warnings found" >&2
}

ran=0
status=0
if [[ -n "${SONUS_SUPABASE_DATABASE_URL:-}" ]]; then
  ran=1
  run_audit "Supabase RLS" "$SONUS_SUPABASE_DATABASE_URL" security/supabase_rls_audit.sql || status=1
fi
if [[ -n "${SONUS_RDS_DATABASE_URL:-}" ]]; then
  ran=1
  run_audit "Sonus RDS role" "$SONUS_RDS_DATABASE_URL" security/rds_role_audit.sql || status=1
fi

if [[ "$ran" == 0 ]]; then
  echo "Set SONUS_SUPABASE_DATABASE_URL and/or SONUS_RDS_DATABASE_URL." >&2
  exit 2
fi
exit "$status"
