#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

target="${1:?usage: materialize_elenkos_rykshaw_20260821.sh TARGET_DIR}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
payload="$root/scripts/ops/elenkos_rykshaw_source_e86e8ab.tar.gz.b64"
digest_file="$root/scripts/ops/elenkos_rykshaw_source_e86e8ab.sha256"
archive="$(mktemp "${RUNNER_TEMP:-/tmp}/elenkos-rykshaw.XXXXXX.tar.gz")"
cleanup() { rm -f "$archive"; }
trap cleanup EXIT

test -s "$payload"
test -s "$digest_file"
base64 --decode "$payload" > "$archive"
expected="$(awk '{print $1}' "$digest_file")"
actual="$(sha256sum "$archive" | awk '{print $1}')"
[[ "$expected" =~ ^[0-9a-f]{64}$ ]]
[[ "$actual" == "$expected" ]]

rm -rf "$target"
mkdir -p "$target"
tar -xzf "$archive" -C "$target" --strip-components=1 --no-same-owner --no-same-permissions

test -s "$target/pubspec.yaml"
test -s "$target/tool/validate.sh"
test ! -e "$target/.git"
! grep -RIlE '(ghp_[A-Za-z0-9]{20,}|lin_api_[A-Za-z0-9]{20,})' "$target" >/dev/null
printf 'RYKSHAW_PAYLOAD_MATERIALIZED source_commit=e86e8ab84008deba79b2ae262b829f3dbe0d532b sha256=%s target=%s\n' "$actual" "$target"
