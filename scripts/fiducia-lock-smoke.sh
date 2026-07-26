#!/usr/bin/env bash
#
# Smoke-test the fiducia lock wire format against a LIVE fiducia before relying
# on the distributed leases in src/coordination.rs.
#
# The message shapes in coordination.rs were cross-checked against
# fiducia-node.rs/src/locks.rs (AcquireBody / RenewBody / ReleaseBody) and the
# documented response envelope. What has NOT been exercised end to end is auth
# and network behaviour: that the key actually carries `locks:write`, that the
# load balancer routes to a node, and that the fencing token round-trips. This
# script does exactly that, in the same order coordination.rs uses at runtime:
# acquire (non-blocking) -> renew -> release.
#
#   FIDUCIA_URL=http://fiducia-load-balance.fiducia.svc.cluster.local:8088 \
#   FIDUCIA_LOCKS_API_KEY=<a locks:write key> \
#   scripts/fiducia-lock-smoke.sh
#
# Read-only against the org: it takes one lock on a throwaway key under
# daedalus/fab/smoke/ and releases it. It never touches a real job key.
#
# Exit 0 = the lease lifecycle works with this key. A 403 insufficient_scope
# means the key is KV-shaped, not lock-shaped — mint one with `locks:write`.
set -euo pipefail

: "${FIDUCIA_URL:?set FIDUCIA_URL to the fiducia base URL}"
: "${FIDUCIA_LOCKS_API_KEY:?set FIDUCIA_LOCKS_API_KEY to a locks:write key}"

base="${FIDUCIA_URL%/}"
key="daedalus/fab/smoke/$(date +%s)-$$"
holder="dd-fab-smoke-$$"
ttl_ms=30000

hdr=(-H "authorization: Bearer ${FIDUCIA_LOCKS_API_KEY}" -H "content-type: application/json")

# jq is used only to read fields back; fall back to python if it is absent.
extract() { # extract <json> <dotted.path>
  if command -v jq >/dev/null 2>&1; then
    printf '%s' "$1" | jq -r ".$2 // empty"
  else
    printf '%s' "$1" | python3 -c "import sys,json;d=json.load(sys.stdin)
p='$2'.split('.')
for k in p:
    d = d.get(k) if isinstance(d,dict) else None
print('' if d is None else d)"
  fi
}

say() { printf '\n=== %s\n' "$*"; }

say "acquire (non-blocking) $key"
acq=$(curl -fsS "${hdr[@]}" -X POST "${base}/v1/locks/acquire" \
  -d "{\"keys\":[\"${key}\"],\"holder\":\"${holder}\",\"request_id\":\"smoke-$$\",\"ttl_ms\":${ttl_ms}}") \
  || { echo "acquire failed — if this was 403 insufficient_scope, the key lacks locks:write" >&2; exit 1; }
acquired=$(extract "$acq" "result.output.acquired")
token=$(extract "$acq" "result.output.fencing_token")
echo "  acquired=${acquired} fencing_token=${token}"
[ "$acquired" = "true" ] || { echo "did not acquire (queued or refused)" >&2; exit 1; }
[ -n "$token" ] || { echo "no fencing token in the acquire response" >&2; exit 1; }

say "renew (must preserve the same fencing token)"
ren=$(curl -fsS "${hdr[@]}" -X POST "${base}/v1/locks/renew" \
  -d "{\"keys\":[\"${key}\"],\"holder\":\"${holder}\",\"fencing_token\":${token},\"ttl_ms\":${ttl_ms}}")
renewed=$(extract "$ren" "result.output.renewed")
echo "  renewed=${renewed}"
[ "$renewed" = "true" ] || { echo "renew was refused; coordination.rs treats that as lost authority" >&2; exit 1; }

say "release (holder + fencing token only, no key)"
rel=$(curl -fsS "${hdr[@]}" -X POST "${base}/v1/locks/release" \
  -d "{\"holder\":\"${holder}\",\"fencing_token\":${token}}")
released=$(extract "$rel" "result.output.released")
echo "  released=${released}"
[ "$released" = "true" ] || { echo "release was refused" >&2; exit 1; }

say "OK — acquire/renew/release round-trips; this key is lock-capable"
