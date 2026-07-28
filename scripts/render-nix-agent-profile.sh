#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
implementation="$script_dir/../nix-agent-profiles/render-nix-agent-profile-impl.sh"
output_dir=""

arguments=("$@")
for ((index = 0; index < ${#arguments[@]}; index++)); do
  case "${arguments[$index]}" in
    --output-dir)
      if ((index + 1 < ${#arguments[@]})); then
        output_dir="${arguments[$((index + 1))]}"
      fi
      ;;
    --output-dir=*)
      output_dir="${arguments[$index]#--output-dir=}"
      ;;
  esac
done

if [[ ! -r "$implementation" ]]; then
  printf 'renderer implementation is missing or unreadable: %s\n' "$implementation" >&2
  exit 69
fi

if ! command -v nixfmt >/dev/null 2>&1; then
  printf '%s\n' 'nixfmt is required to normalize generated Nix files' >&2
  exit 69
fi

bash "$implementation" "$@"

if [[ -z "$output_dir" ]]; then
  printf '%s\n' 'renderer completed without a resolvable --output-dir' >&2
  exit 70
fi

output_dir="$(cd -- "$output_dir" && pwd)"
nixfmt \
  "$output_dir/flake.nix" \
  "$output_dir/.nix/profile-packages.nix" \
  "$output_dir/.nix/dev-shell.nix"
