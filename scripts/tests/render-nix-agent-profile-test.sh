#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
renderer="$repo_root/scripts/render-nix-agent-profile.sh"
profiles_file="$repo_root/nix-agent-profiles/profiles.json"
tmp_root="$(mktemp -d)"
trap 'rm -rf "$tmp_root"' EXIT

jq -e '
  .schemaVersion == 1
  and (.profiles | type == "object")
  and (["rust", "flutter", "node", "go", "python", "kubernetes", "polyglot"] - (.profiles | keys) | length == 0)
  and all(.profiles[];
    (.description | type == "string" and length > 0)
    and (.packages | type == "array" and length > 0)
    and (.commands | type == "array" and length > 0)
    and (.requiresLocalHook | type == "boolean")
    and (.ociPosture | type == "string" and length > 0)
  )
' "$profiles_file" >/dev/null

while IFS= read -r profile; do
  target="$tmp_root/$profile"
  mkdir -p "$target"
  git -C "$target" init -q

  bash "$renderer" --profile "$profile" --output-dir "$target" --skip-lock

  for expected_path in \
    flake.nix \
    .nix/profile-packages.nix \
    .nix/dev-shell.nix \
    .nix/agent-check.sh \
    .nix/README.md \
    .github/workflows/nix.yml \
    .gitignore; do
    test -f "$target/$expected_path"
  done

  test -x "$target/.nix/agent-check.sh"
  bash -n "$target/.nix/agent-check.sh"
  shellcheck "$target/.nix/agent-check.sh"
  shfmt -i 2 -ci -d "$target/.nix/agent-check.sh"
  nixfmt --check \
    "$target/flake.nix" \
    "$target/.nix/profile-packages.nix" \
    "$target/.nix/dev-shell.nix"
  ruby -e 'require "yaml"; YAML.load_file(ARGV.fetch(0))' "$target/.github/workflows/nix.yml"

  grep -Fq 'actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1' "$target/.github/workflows/nix.yml"
  grep -Fq 'cachix/install-nix-action@630ae543ea3a38a9a4166f03376c02c50f408342' "$target/.github/workflows/nix.yml"
  grep -Fq '.cache/nix-agent/' "$target/.gitignore"
  ! grep -RInE 'AWS_PROFILE|GOOGLE_APPLICATION_CREDENTIALS|KUBECONFIG=' \
    "$target/flake.nix" "$target/.nix" "$target/.github/workflows/nix.yml"

  first_package="$(jq -r --arg profile "$profile" '.profiles[$profile].packages[0]' "$profiles_file")"
  grep -Fq "$first_package" "$target/.nix/profile-packages.nix"

  requires_local_hook="$(jq -r --arg profile "$profile" '.profiles[$profile].requiresLocalHook' "$profiles_file")"
  if [[ "$requires_local_hook" == "true" ]]; then
    if bash "$target/.nix/agent-check.sh" local; then
      printf 'profile %s unexpectedly accepted a missing required local hook\n' "$profile" >&2
      exit 1
    fi
  else
    bash "$target/.nix/agent-check.sh" local
  fi

  if bash "$renderer" --profile "$profile" --output-dir "$target" --skip-lock; then
    printf 'profile %s unexpectedly overwrote managed files without --force\n' "$profile" >&2
    exit 1
  fi

  bash "$renderer" --profile "$profile" --output-dir "$target" --skip-lock --force
  test "$(grep -Fc '# BEGIN managed nix-agent profile' "$target/.gitignore")" -eq 1
done < <(jq -r '.profiles | keys[]' "$profiles_file")

if bash "$renderer" --profile unknown --output-dir "$tmp_root/unknown" --skip-lock; then
  printf '%s\n' 'unknown profile unexpectedly rendered' >&2
  exit 1
fi

printf '%s\n' 'all Nix agent profile fixtures passed'
