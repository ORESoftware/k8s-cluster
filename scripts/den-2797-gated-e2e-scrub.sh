#!/usr/bin/env bash
set -Eeuo pipefail

work=/tmp/den-2797-gated-e2e
for path in \
  "$work/owner.private.pem" \
  "$work/owner.public.pem" \
  "$work/owner.ciphertext.b64" \
  "$work/owner.ciphertext.bin" \
  "$work/owner.token" \
  "$work/git-askpass.sh"; do
  if [[ -f "$path" ]]; then
    if command -v shred >/dev/null 2>&1; then
      shred --force --iterations=2 --zero --remove "$path" || rm -f "$path"
    else
      rm -f "$path"
    fi
  fi
done
rm -rf "$work"
echo "local gated E2E credential and work material scrubbed"
