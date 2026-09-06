#!/usr/bin/env bash
# End-to-end smoke test for the Bash client, mirroring clients/python/smoke.py.
#
#   ./smoke.sh
#
# Override the broker endpoint via LIVE_MUTEX_HOST / LIVE_MUTEX_PORT. Start a
# broker first, e.g.:
#   cargo run --release --no-default-features --bin dd-rust-network-mutex

set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=live_mutex_client.sh
. "$HERE/live_mutex_client.sh"

HOST="${LIVE_MUTEX_HOST:-127.0.0.1}"
PORT="${LIVE_MUTEX_PORT:-6970}"

lmx_connect "$HOST" "$PORT" "${LIVE_MUTEX_TOKEN:-}"
echo "[smoke-shell] connected ${HOST}:${PORT}"
trap lmx_disconnect EXIT

# Exclusive lock
lmx_acquire "smoke-shell-exclusive" 5000
echo "[smoke-shell] exclusive grant: lockUuid=${LMX_LOCK_UUID} fencing=${LMX_FENCE}"
lmx_release "smoke-shell-exclusive" "$LMX_LOCK_UUID"
echo "[smoke-shell] released exclusive"

# Fail-fast try-lock: second attempt on a held key must report contention
lmx_acquire "smoke-shell-try" 5000
held="$LMX_LOCK_UUID"
if lmx_try_acquire "smoke-shell-try" 5000; then
  echo "[smoke-shell] FAIL: try_acquire granted a held key" >&2; exit 1
fi
echo "[smoke-shell] try-lock correctly refused a held key"
lmx_release "smoke-shell-try" "$held"

# Composite (multi-key) lock
lmx_acquire_many 5000 -- smoke-shell-a smoke-shell-b smoke-shell-c
echo "[smoke-shell] composite grant: lockUuid=${LMX_LOCK_UUID} tokens=${LMX_FENCES}"
lmx_release_many "$LMX_LOCK_UUID" -- smoke-shell-a smoke-shell-b smoke-shell-c
echo "[smoke-shell] released composite"

# Reader / writer locks
lmx_acquire_write "smoke-shell-rw"
echo "[smoke-shell] writer grant: fencing=${LMX_FENCE}"
lmx_release_write "smoke-shell-rw"
lmx_acquire_read "smoke-shell-rw"
echo "[smoke-shell] reader grant: fencing=${LMX_FENCE}"
lmx_release_read "smoke-shell-rw"

# Introspection
lmx_ls
echo "[smoke-shell] ls keys=${LMX_KEYS}"

echo "[smoke-shell] OK"
