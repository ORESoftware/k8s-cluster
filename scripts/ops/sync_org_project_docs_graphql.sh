#!/usr/bin/env bash
set -Eeuo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
cat "$here/.org-project-docs-graphql"/part-*.b64 | tr -d '\r\n' | base64 -d > "$tmp"
printf '%s  %s\n' 'b9e2519c6684064faacd16495ead7326888b5a74346b614d22c2b7c03e80b1cb' "$tmp" | sha256sum --check --status
bash "$tmp" "$@"
