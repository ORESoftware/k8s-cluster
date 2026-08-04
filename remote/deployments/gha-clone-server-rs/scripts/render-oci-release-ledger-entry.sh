#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 5 ]]; then
  printf 'usage: %s REPOSITORY SOURCE_SHA TARGET IMAGE DIGEST\n' "$0" >&2
  exit 64
fi

repository="$1"
source_sha="$2"
target="$3"
image="$4"
digest="$5"

if [[ "$repository" != 'ORESoftware/k8s-cluster' ]]; then
  printf 'unexpected source repository\n' >&2
  exit 65
fi
if [[ ! "$source_sha" =~ ^[0-9a-f]{40}$ ]]; then
  printf 'invalid source SHA\n' >&2
  exit 65
fi
if [[ ! "$digest" =~ ^sha256:[0-9a-f]{64}$ ]]; then
  printf 'invalid OCI digest\n' >&2
  exit 65
fi

case "$target" in
  clone-server)
    expected_image='ghcr.io/oresoftware/gha-clone-server'
    ;;
  executor-router)
    expected_image='ghcr.io/oresoftware/gha-executor-router'
    ;;
  *)
    printf 'unexpected Docker target\n' >&2
    exit 65
    ;;
esac

if [[ "$image" != "$expected_image" ]]; then
  printf 'image does not match Docker target\n' >&2
  exit 65
fi

immutable_ref="${image}@${digest}"
printf '<!-- gha-continuity-oci-release:%s:%s -->\n' "$source_sha" "$target"
printf '```json\n'
printf '{"schema_version":1,"repository":"%s","source_sha":"%s","target":"%s","image":"%s","digest":"%s","ref":"%s"}\n' \
  "$repository" "$source_sha" "$target" "$image" "$digest" "$immutable_ref"
printf '```\n'
