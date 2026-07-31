#!/usr/bin/env bash
#
# Single-source management for the checked-in IDE MCP configs that point at the
# dd-remote-gateway.
#
# Why not symlink them to one file? Because the three IDE configs are NOT
# format-compatible (verified against the tools' docs):
#   - .mcp.json            (Claude Code) : {"mcpServers":...}, token as ${DD_MCP_TOKEN}
#   - .cursor/mcp.json     (Cursor)      : {"mcpServers":...}, token as ${env:DD_MCP_TOKEN}
#   - .vscode/mcp.json     (VS Code)     : {"inputs":[...], "servers":...}, prompted token
#   - mcp/codex-config.example.toml (Codex) : TOML, literal token placeholder
# Claude Code and Cursor have no common env-var interpolation syntax (each leaves
# the other's form as a literal, breaking auth), Cursor does not read a root
# .mcp.json, VS Code uses a different schema, and Claude Code ignores an mcp.json
# placed inside .claude/. A shared/symlinked file therefore cannot carry a working
# token reference for all of them. See mcp/README.md.
#
# What IS shared is the gateway host (the EC2 Elastic IP). This script treats that
# host as the single source of truth across the per-tool files:
#
#   mcp-config.sh check            # read-only: assert every config uses the same host
#   mcp-config.sh check --live     # also compare against the live EC2 EIP (needs awscli)
#   mcp-config.sh set-ip <NEW_IP>  # surgically swap the host across all files (+ README)
#   mcp-config.sh set-ip <NEW_IP> --dry-run
#
# `check` never writes. `set-ip` only rewrites the host substring, preserving all
# other formatting/comments, and prints a diff for review before you commit.
set -euo pipefail

repo_root() {
  local here
  here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  git -C "$here" rev-parse --show-toplevel 2>/dev/null || dirname "$here"
}
ROOT="$(repo_root)"

# Files that embed the gateway URL. The IDE JSON configs are the authoritative set
# checked for agreement; README + codex example are updated by set-ip too.
CONFIG_FILES=(
  ".mcp.json"
  ".cursor/mcp.json"
  ".vscode/mcp.json"
  "mcp/codex-config.example.toml"
)
DOC_FILES=(
  "mcp/README.md"
)

# Extract the gateway authority (host[:port]) from the first https URL in a file.
extract_host() {
  local f="$1"
  [ -f "$ROOT/$f" ] || return 0
  grep -oE 'https://[^/"[:space:]]+' "$ROOT/$f" 2>/dev/null \
    | sed -E 's#https://##' | sort -u
}

cmd_check() {
  local live=0
  [ "${1:-}" = "--live" ] && live=1

  # No associative arrays: keep this portable to macOS's stock bash 3.2.
  local all_hosts=""
  local missing=0
  for f in "${CONFIG_FILES[@]}"; do
    if [ ! -f "$ROOT/$f" ]; then
      echo "  MISSING  $f"
      missing=1
      continue
    fi
    all_hosts+="$(extract_host "$f" | tr '\n' ' ') "
  done

  # Distinct hosts across every config file.
  local distinct
  distinct="$(printf '%s\n' $all_hosts | sed '/^$/d' | sort -u)"
  local n
  n="$(printf '%s\n' "$distinct" | sed '/^$/d' | wc -l | tr -d ' ')"

  if [ "$n" -gt 1 ]; then
    echo "DRIFT: IDE MCP configs reference more than one gateway host:"
    for f in "${CONFIG_FILES[@]}"; do
      [ -f "$ROOT/$f" ] || continue
      printf '  %-32s %s\n' "$f" "$(extract_host "$f" | tr '\n' ' ')"
    done
    return 1
  fi

  local host="$distinct"
  if [ -z "$host" ]; then
    echo "No gateway host found in any config (nothing to check)."
    return 0
  fi
  echo "OK: all IDE MCP configs point at $host"
  [ "$missing" -eq 1 ] && { echo "  (one or more configs missing — see above)"; return 1; }

  if [ "$live" -eq 1 ]; then
    check_live "$host"
  fi
  return 0
}

check_live() {
  local host="$1"
  if ! command -v aws >/dev/null 2>&1; then
    echo "  --live: awscli not available; skipping live EIP comparison."
    return 0
  fi
  local live_ip
  live_ip="$(aws ec2 describe-instances --region us-east-1 \
    --filters Name=tag:Name,Values=dd-remote-k8s-1 Name=instance-state-name,Values=running \
    --query 'Reservations[].Instances[].PublicIpAddress' --output text 2>/dev/null | tr '\t' '\n' | sed '/^$/d' | head -1 || true)"
  if [ -z "$live_ip" ]; then
    echo "  --live: could not resolve live EIP (no creds / instance down?); skipped."
    return 0
  fi
  # host may include a :port; compare the address portion only.
  local host_ip="${host%%:*}"
  if [ "$host_ip" = "$live_ip" ]; then
    echo "  live EIP matches configs: $live_ip"
  else
    echo "DRIFT: configs use $host_ip but the live EIP is $live_ip"
    echo "  fix with: mcp/mcp-config.sh set-ip $live_ip"
    return 1
  fi
}

cmd_set_ip() {
  local new_ip="${1:-}"
  local dry=0
  [ "${2:-}" = "--dry-run" ] && dry=1
  if [ -z "$new_ip" ]; then
    echo "usage: mcp-config.sh set-ip <NEW_IP> [--dry-run]" >&2
    return 2
  fi

  # Current host = the agreed-upon one from the config files.
  local current
  current="$(for f in "${CONFIG_FILES[@]}"; do extract_host "$f"; done | sort -u | sed '/^$/d' | head -1)"
  local current_ip="${current%%:*}"
  if [ -z "$current_ip" ]; then
    echo "Could not determine the current gateway host; aborting." >&2
    return 1
  fi
  if [ "$current_ip" = "$new_ip" ]; then
    echo "Gateway host is already $new_ip — nothing to do."
    return 0
  fi

  echo "Replacing gateway host: $current_ip -> $new_ip"
  local targets=( "${CONFIG_FILES[@]}" "${DOC_FILES[@]}" )
  for f in "${targets[@]}"; do
    [ -f "$ROOT/$f" ] || continue
    local hits
    hits="$(grep -c -F "$current_ip" "$ROOT/$f" 2>/dev/null || true)"
    [ "${hits:-0}" -eq 0 ] && continue
    if [ "$dry" -eq 1 ]; then
      printf '  would update %-32s (%s occurrence(s))\n' "$f" "$hits"
    else
      # Literal, in-place replacement; preserves all other formatting/comments.
      perl -pi -e "s/\Q$current_ip\E/$new_ip/g" "$ROOT/$f"
      printf '  updated %-32s (%s occurrence(s))\n' "$f" "$hits"
    fi
  done

  if [ "$dry" -eq 1 ]; then
    echo "(dry run — no files changed)"
  else
    echo
    echo "Done. Review with: git -C \"$ROOT\" diff -- ${targets[*]}"
    echo "NOTE: a new EIP also requires reissuing the gateway TLS cert (see mcp/README.md)."
  fi
}

main() {
  local sub="${1:-check}"
  shift || true
  case "$sub" in
    check)   cmd_check "$@" ;;
    set-ip)  cmd_set_ip "$@" ;;
    -h|--help|help)
      sed -n '2,40p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' ;;
    *)
      echo "unknown subcommand: $sub (try: check | set-ip | help)" >&2
      return 2 ;;
  esac
}
main "$@"
