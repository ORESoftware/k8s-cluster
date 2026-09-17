#!/usr/bin/env bash
# live_mutex_client.sh — Bash client for the dd-rust-network-mutex broker.
#
# Speaks the newline-delimited JSON wire protocol that is the single source of
# truth in ../../PROTOCOL.md (generated from ../../src/protocol.rs). Like the
# other language clients (Python/Go/Rust/…), every wire `type` value lives in a
# named constant below instead of being sprinkled around as a magic string, so
# adding a broker variant means adding a constant here too.
#
# Transport is Bash's built-in /dev/tcp, so the only dependency is bash itself
# (3.2+, compiled with net redirections — the default on macOS and Linux).
# zsh/sh users run the scripts directly: the shebang selects bash. Source this
# file and call the lmx_* functions; see smoke.sh for an end-to-end example.

# The wire constants and the LMX_* result globals are this library's public API:
# callers reference them (and check-protocol-parity.sh greps them), so the
# "appears unused" (SC2034) heuristic is suppressed file-wide here.
# shellcheck disable=SC2034

# ---------------------------------------------------------------------------
# Wire discriminators — mirror the Rust `Request` / `Response` tagged enums.
# (kept here, in one file, so clients/check-protocol-parity.sh can verify them)
# ---------------------------------------------------------------------------

# Request `type` values (src/protocol.rs `enum Request`)
readonly LMX_REQ_VERSION="version"
readonly LMX_REQ_AUTH="auth"
readonly LMX_REQ_LOCK="lock"
readonly LMX_REQ_UNLOCK="unlock"
readonly LMX_REQ_REGISTER_READ="registerRead"
readonly LMX_REQ_REGISTER_WRITE="registerWrite"
readonly LMX_REQ_END_READ="endRead"
readonly LMX_REQ_END_WRITE="endWrite"
readonly LMX_REQ_LOCK_INFO="lockInfo"
readonly LMX_REQ_LS="ls"
readonly LMX_REQ_HEARTBEAT="heartbeat"

# Response `type` values (src/protocol.rs `enum Response`)
readonly LMX_RES_VERSION="version"
readonly LMX_RES_AUTH="auth"
readonly LMX_RES_LOCK="lock"
readonly LMX_RES_COMPOSITE_LOCK="compositeLock"
readonly LMX_RES_UNLOCK="unlock"
readonly LMX_RES_REGISTER_READ_RESULT="registerReadResult"
readonly LMX_RES_REGISTER_WRITE_RESULT="registerWriteResult"
readonly LMX_RES_END_READ_RESULT="endReadResult"
readonly LMX_RES_END_WRITE_RESULT="endWriteResult"
readonly LMX_RES_LOCK_INFO="lockInfo"
readonly LMX_RES_LS_RESULT="lsResult"
readonly LMX_RES_REELECTION="reelection"
readonly LMX_RES_ERROR="error"
readonly LMX_RES_OK="ok"

# Read timeout (whole seconds; Bash read -t granularity).
: "${LMX_TIMEOUT:=30}"

# Populated by the round-trip helpers / op functions:
LMX_REPLY=""       # last raw JSON frame received
LMX_ERROR=""       # last broker error string (when an op fails)
LMX_LOCK_UUID=""   # lock handle from the last successful acquire
LMX_FENCE=""       # fencing token from the last successful acquire/grant
LMX_FENCES=""      # raw fencingTokens object from the last composite grant
LMX_KEYS=""        # raw keys array from the last ls

# ---------------------------------------------------------------------------
# Tiny helpers
# ---------------------------------------------------------------------------

lmx_uuid() {
  if command -v uuidgen >/dev/null 2>&1; then
    uuidgen | tr '[:upper:]' '[:lower:]'
  elif [ -r /proc/sys/kernel/random/uuid ]; then
    cat /proc/sys/kernel/random/uuid
  else
    printf '%s-%s-%s-%s' "$RANDOM$RANDOM" "$RANDOM" "$$" "$(date +%s)"
  fi
}

# Escape a value for safe embedding inside a JSON string literal (backslash and
# double-quote, plus the common control characters). Without this a key like
# `a"b` would break the frame or forge extra fields.
lmx_json_escape() {
  local s=$1
  s=${s//\\/\\\\}
  s=${s//\"/\\\"}
  s=${s//$'\n'/\\n}
  s=${s//$'\r'/\\r}
  s=${s//$'\t'/\\t}
  printf '%s' "$s"
}

# Extract a string field: lmx_json_str <field> <<<"$json"
lmx_json_str() { sed -n "s/.*\"$1\":\"\([^\"]*\)\".*/\1/p"; }
# Extract a numeric field: lmx_json_num <field> <<<"$json"
lmx_json_num() { sed -n "s/.*\"$1\":\([0-9][0-9]*\).*/\1/p"; }

# Build a JSON array literal from positional args (each element escaped):
# lmx_json_array a b c -> ["a","b","c"]
lmx_json_array() {
  local out="" k
  for k in "$@"; do out="$out,\"$(lmx_json_escape "$k")\""; done
  printf '[%s]' "${out:1}"
}

# ---------------------------------------------------------------------------
# Connection (one multiplexed TCP/UDS stream on fd 3)
# ---------------------------------------------------------------------------

# lmx_connect <host> <port> [token]   (TCP)
# lmx_connect_uds <path> [token]      (Unix domain socket)
lmx_connect() {
  local host="${1:-127.0.0.1}" port="${2:-6970}" token="${3:-}"
  exec 3<>"/dev/tcp/${host}/${port}" || { LMX_ERROR="connect ${host}:${port} failed"; return 1; }
  _lmx_after_connect "$token"
}

lmx_connect_uds() {
  local path="$1" token="${2:-}"
  exec 3<>"$path" || { LMX_ERROR="connect ${path} failed"; return 1; }
  _lmx_after_connect "$token"
}

_lmx_after_connect() {
  local token="$1"
  [ -z "$token" ] && return 0
  local uuid; uuid="$(lmx_uuid)"
  _lmx_send "$(printf '{"type":"%s","uuid":"%s","token":"%s"}' \
    "$LMX_REQ_AUTH" "$uuid" "$(lmx_json_escape "$token")")"
  _lmx_read_reply "$uuid" || { LMX_ERROR="auth: no reply"; return 1; }
  case "$LMX_REPLY" in
    *'"ok":true'*) return 0 ;;
    *) LMX_ERROR="auth rejected: $LMX_REPLY"; return 1 ;;
  esac
}

lmx_disconnect() { exec 3>&- 2>/dev/null; exec 3<&- 2>/dev/null; return 0; }

_lmx_send() { printf '%s\n' "$1" >&3; }

# Read frames until one carries our uuid; stash it in LMX_REPLY.
_lmx_read_reply() {
  local want="$1" line
  while IFS= read -r -t "$LMX_TIMEOUT" line <&3; do
    [ -z "$line" ] && continue
    case "$line" in *"\"uuid\":\"$want\""*) LMX_REPLY="$line"; return 0 ;; esac
  done
  return 1
}

# Like _lmx_read_reply but skips queued (acquired:false, no error) notices so a
# blocking acquire drains until the real grant or an error frame.
_lmx_read_grant() {
  local want="$1" line
  while IFS= read -r -t "$LMX_TIMEOUT" line <&3; do
    [ -z "$line" ] && continue
    case "$line" in *"\"uuid\":\"$want\""*) ;; *) continue ;; esac
    case "$line" in
      *'"acquired":true'*)  LMX_REPLY="$line"; return 0 ;;
      *'"error":'*)         LMX_REPLY="$line"; return 0 ;;
      *'"acquired":false'*) continue ;;   # queued notice; keep waiting
      *)                    LMX_REPLY="$line"; return 0 ;;
    esac
  done
  return 1
}

# Read until an RW grant (granted:true) arrives for our uuid.
_lmx_read_until_granted() {
  local want="$1" line
  while IFS= read -r -t "$LMX_TIMEOUT" line <&3; do
    [ -z "$line" ] && continue
    case "$line" in *"\"uuid\":\"$want\""*) ;; *) continue ;; esac
    case "$line" in
      *'"granted":true'*) LMX_REPLY="$line"; return 0 ;;
      *'"error":'*)       LMX_REPLY="$line"; return 1 ;;
    esac
  done
  return 1
}

# ---------------------------------------------------------------------------
# Exclusive / semaphore locks
# ---------------------------------------------------------------------------

# lmx_acquire <key> [ttl_ms] [max]  -> 0 + LMX_LOCK_UUID/LMX_FENCE, blocks until granted
lmx_acquire() {
  local key="$1" ttl="${2:-0}" max="${3:-}" uuid; uuid="$(lmx_uuid)"
  local maxf=""; [ -n "$max" ] && maxf=",\"max\":$max"
  _lmx_send "$(printf '{"type":"%s","uuid":"%s","key":"%s","ttl":%s,"wait":true%s}' \
    "$LMX_REQ_LOCK" "$uuid" "$(lmx_json_escape "$key")" "$ttl" "$maxf")"
  _lmx_read_grant "$uuid" || { LMX_ERROR="acquire($key): timeout"; return 1; }
  case "$LMX_REPLY" in *'"acquired":true'*) ;; *) LMX_ERROR="acquire($key): $LMX_REPLY"; return 1 ;; esac
  LMX_LOCK_UUID="$(lmx_json_str lockUuid <<<"$LMX_REPLY")"
  LMX_FENCE="$(lmx_json_num fencingToken <<<"$LMX_REPLY")"
}

# lmx_try_acquire <key> [ttl_ms] [max] -> 0 granted / 2 contended / 1 error
lmx_try_acquire() {
  local key="$1" ttl="${2:-0}" max="${3:-}" uuid; uuid="$(lmx_uuid)"
  local maxf=""; [ -n "$max" ] && maxf=",\"max\":$max"
  _lmx_send "$(printf '{"type":"%s","uuid":"%s","key":"%s","ttl":%s,"wait":false%s}' \
    "$LMX_REQ_LOCK" "$uuid" "$(lmx_json_escape "$key")" "$ttl" "$maxf")"
  _lmx_read_reply "$uuid" || { LMX_ERROR="try_acquire($key): timeout"; return 1; }
  case "$LMX_REPLY" in
    *'"error":'*)         LMX_ERROR="try_acquire($key): $LMX_REPLY"; return 1 ;;
    *'"acquired":true'*)  LMX_LOCK_UUID="$(lmx_json_str lockUuid <<<"$LMX_REPLY")"
                          LMX_FENCE="$(lmx_json_num fencingToken <<<"$LMX_REPLY")"; return 0 ;;
    *)                    return 2 ;;   # contended
  esac
}

# lmx_release <key> <lock_uuid>
lmx_release() {
  local key="$1" lock="$2" uuid; uuid="$(lmx_uuid)"
  _lmx_send "$(printf '{"type":"%s","uuid":"%s","key":"%s","lockUuid":"%s"}' \
    "$LMX_REQ_UNLOCK" "$uuid" "$(lmx_json_escape "$key")" "$(lmx_json_escape "$lock")")"
  _lmx_read_reply "$uuid" || { LMX_ERROR="release($key): timeout"; return 1; }
  case "$LMX_REPLY" in *'"unlocked":true'*) return 0 ;; *) LMX_ERROR="release($key): $LMX_REPLY"; return 1 ;; esac
}

# lmx_force_unlock <key>
lmx_force_unlock() {
  local key="$1" uuid; uuid="$(lmx_uuid)"
  _lmx_send "$(printf '{"type":"%s","uuid":"%s","key":"%s","force":true}' \
    "$LMX_REQ_UNLOCK" "$uuid" "$(lmx_json_escape "$key")")"
  _lmx_read_reply "$uuid" || { LMX_ERROR="force_unlock($key): timeout"; return 1; }
  case "$LMX_REPLY" in *'"error":'*) LMX_ERROR="force_unlock($key): $LMX_REPLY"; return 1 ;; *) return 0 ;; esac
}

# ---------------------------------------------------------------------------
# Composite (multi-key) locks — up to 5 keys, broker sorts for deadlock-freedom
# ---------------------------------------------------------------------------

# lmx_acquire_many [ttl_ms] -- <key>...   -> LMX_LOCK_UUID / LMX_FENCES
lmx_acquire_many() {
  local ttl="$1"; shift; [ "${1:-}" = "--" ] && shift
  local uuid; uuid="$(lmx_uuid)"
  _lmx_send "$(printf '{"type":"%s","uuid":"%s","keys":%s,"ttl":%s,"wait":true}' \
    "$LMX_REQ_LOCK" "$uuid" "$(lmx_json_array "$@")" "$ttl")"
  _lmx_read_grant "$uuid" || { LMX_ERROR="acquire_many: timeout"; return 1; }
  case "$LMX_REPLY" in *'"acquired":true'*) ;; *) LMX_ERROR="acquire_many: $LMX_REPLY"; return 1 ;; esac
  LMX_LOCK_UUID="$(lmx_json_str lockUuid <<<"$LMX_REPLY")"
  LMX_FENCES="$(sed -n 's/.*\("fencingTokens":{[^}]*}\).*/\1/p' <<<"$LMX_REPLY")"
}

# lmx_release_many <lock_uuid> -- <key>...
lmx_release_many() {
  local lock="$1"; shift; [ "${1:-}" = "--" ] && shift
  local uuid; uuid="$(lmx_uuid)"
  _lmx_send "$(printf '{"type":"%s","uuid":"%s","keys":%s,"lockUuid":"%s"}' \
    "$LMX_REQ_UNLOCK" "$uuid" "$(lmx_json_array "$@")" "$(lmx_json_escape "$lock")")"
  _lmx_read_reply "$uuid" || { LMX_ERROR="release_many: timeout"; return 1; }
  case "$LMX_REPLY" in *'"unlocked":true'*) return 0 ;; *) LMX_ERROR="release_many: $LMX_REPLY"; return 1 ;; esac
}

# ---------------------------------------------------------------------------
# Reader / writer locks
# ---------------------------------------------------------------------------

lmx_acquire_read() {
  local key="$1" uuid; uuid="$(lmx_uuid)"
  _lmx_send "$(printf '{"type":"%s","uuid":"%s","key":"%s"}' \
    "$LMX_REQ_REGISTER_READ" "$uuid" "$(lmx_json_escape "$key")")"
  _lmx_read_until_granted "$uuid" || { LMX_ERROR="acquire_read($key): not granted"; return 1; }
  LMX_FENCE="$(lmx_json_num fencingToken <<<"$LMX_REPLY")"
}

lmx_acquire_write() {
  local key="$1" uuid; uuid="$(lmx_uuid)"
  _lmx_send "$(printf '{"type":"%s","uuid":"%s","key":"%s"}' \
    "$LMX_REQ_REGISTER_WRITE" "$uuid" "$(lmx_json_escape "$key")")"
  _lmx_read_until_granted "$uuid" || { LMX_ERROR="acquire_write($key): not granted"; return 1; }
  LMX_FENCE="$(lmx_json_num fencingToken <<<"$LMX_REPLY")"
}

lmx_release_read() {
  local key="$1" uuid; uuid="$(lmx_uuid)"
  _lmx_send "$(printf '{"type":"%s","uuid":"%s","key":"%s"}' \
    "$LMX_REQ_END_READ" "$uuid" "$(lmx_json_escape "$key")")"
  _lmx_read_reply "$uuid"
}

lmx_release_write() {
  local key="$1" uuid; uuid="$(lmx_uuid)"
  _lmx_send "$(printf '{"type":"%s","uuid":"%s","key":"%s"}' \
    "$LMX_REQ_END_WRITE" "$uuid" "$(lmx_json_escape "$key")")"
  _lmx_read_reply "$uuid"
}

# ---------------------------------------------------------------------------
# Introspection / keepalive
# ---------------------------------------------------------------------------

# lmx_ls -> LMX_KEYS holds the raw keys array
lmx_ls() {
  local uuid; uuid="$(lmx_uuid)"
  _lmx_send "$(printf '{"type":"%s","uuid":"%s"}' "$LMX_REQ_LS" "$uuid")"
  _lmx_read_reply "$uuid" || { LMX_ERROR="ls: timeout"; return 1; }
  LMX_KEYS="$(sed -n 's/.*\("keys":\[[^]]*\]\).*/\1/p' <<<"$LMX_REPLY")"
}

# lmx_lock_info <key> -> LMX_REPLY holds the raw lockInfo frame
lmx_lock_info() {
  local key="$1" uuid; uuid="$(lmx_uuid)"
  _lmx_send "$(printf '{"type":"%s","uuid":"%s","key":"%s"}' \
    "$LMX_REQ_LOCK_INFO" "$uuid" "$(lmx_json_escape "$key")")"
  _lmx_read_reply "$uuid" || { LMX_ERROR="lock_info($key): timeout"; return 1; }
}

# lmx_heartbeat — fire-and-forget keepalive (broker no-ops it)
lmx_heartbeat() {
  local uuid; uuid="$(lmx_uuid)"
  _lmx_send "$(printf '{"type":"%s","uuid":"%s"}' "$LMX_REQ_HEARTBEAT" "$uuid")"
}
