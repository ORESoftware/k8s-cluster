#!/usr/bin/env bash
set -euo pipefail

script="scripts/ci/init-required-submodules.sh"
if [[ ! -f "$script" ]]; then
  echo "missing $script" >&2
  exit 1
fi
bash -n "$script"

mapfile -t libs < <(bash "$script" --list remote/libs)
if ((${#libs[@]} != 1)) || [[ "${libs[0]}" != "remote/libs" ]]; then
  printf 'remote/libs expansion was unexpected: %s\n' "${libs[*]}" >&2
  exit 1
fi

mapfile -t deployments < <(bash "$script" --list remote/deployments)
if ((${#deployments[@]} < 20)); then
  echo "expected at least 20 deployment gitlinks, found ${#deployments[@]}" >&2
  exit 1
fi
for path in "${deployments[@]}"; do
  if [[ "$path" != remote/deployments/* ]]; then
    echo "deployment selector escaped its prefix: $path" >&2
    exit 1
  fi
done

error_file="$(mktemp)"
trap 'rm -f "$error_file"' EXIT
if bash "$script" --list remote/does-not-exist > /dev/null 2>"$error_file"; then
  echo "unknown selector unexpectedly succeeded" >&2
  exit 1
fi
if ! grep -Fq 'selector matched no declared submodules' "$error_file"; then
  echo "unknown selector did not produce the precise preflight error" >&2
  cat "$error_file" >&2
  exit 1
fi

echo "submodule bootstrap preflight contract passed"
