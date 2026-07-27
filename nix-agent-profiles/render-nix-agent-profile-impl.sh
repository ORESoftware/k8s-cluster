#!/usr/bin/env bash
set -euo pipefail

# jq variables are deliberately passed with --arg and referenced inside
# single-quoted jq programs; shell interpolation would be incorrect.
# shellcheck disable=SC2016

usage() {
  cat <<'USAGE'
Usage:
  bash scripts/render-nix-agent-profile.sh \
    --profile <rust|flutter|node|go|python|kubernetes|polyglot> \
    --output-dir <repository-root> [--force] [--skip-lock]

The renderer writes a root flake, `.nix/` implementation, pinned Nix CI, and a
narrow `.gitignore` block. By default it also runs `nix flake lock` in the target
repository. `--skip-lock` exists only for offline fixture tests and leaves the
target incomplete until a lock file is generated and committed.
USAGE
}

profile=""
output_dir=""
force=0
skip_lock=0

while (($# > 0)); do
  case "$1" in
    --profile)
      profile="${2:-}"
      shift 2
      ;;
    --output-dir)
      output_dir="${2:-}"
      shift 2
      ;;
    --force)
      force=1
      shift
      ;;
    --skip-lock)
      skip_lock=1
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      printf 'unknown argument: %s\n' "$1" >&2
      usage >&2
      exit 64
      ;;
  esac
done

if [[ -z "$profile" || -z "$output_dir" ]]; then
  usage >&2
  exit 64
fi

for command_name in jq install; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    printf 'required command is unavailable: %s\n' "$command_name" >&2
    exit 69
  fi
done

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/.." && pwd)"
profiles_file="$repo_root/nix-agent-profiles/profiles.json"

if ! jq -e --arg profile "$profile" '.profiles[$profile] != null' "$profiles_file" >/dev/null; then
  printf 'unknown profile: %s\n' "$profile" >&2
  printf 'available profiles:\n' >&2
  jq -r '.profiles | keys[] | "  - \(.)"' "$profiles_file" >&2
  exit 64
fi

output_dir="$(mkdir -p -- "$output_dir" && cd -- "$output_dir" && pwd)"
description="$(jq -r --arg profile "$profile" '.profiles[$profile].description' "$profiles_file")"
requires_local_hook="$(jq -r --arg profile "$profile" '.profiles[$profile].requiresLocalHook' "$profiles_file")"
oci_posture="$(jq -r --arg profile "$profile" '.profiles[$profile].ociPosture' "$profiles_file")"

managed_files=(
  flake.nix
  .nix/profile-packages.nix
  .nix/dev-shell.nix
  .nix/agent-check.sh
  .nix/README.md
  .github/workflows/nix.yml
)

if ((force == 0)); then
  for relative_path in "${managed_files[@]}"; do
    if [[ -e "$output_dir/$relative_path" ]]; then
      printf 'refusing to overwrite %s; inspect it and rerun with --force if appropriate\n' "$relative_path" >&2
      exit 73
    fi
  done
fi

mkdir -p -- "$output_dir/.nix" "$output_dir/.github/workflows"

cat >"$output_dir/flake.nix" <<'NIX'
{
  description = "Agent-first repository development environment";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    {
      self,
      nixpkgs,
      ...
    }:
    let
      systems = [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-darwin"
        "x86_64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      pkgsFor = system: import nixpkgs { inherit system; };
    in
    {
      formatter = forAllSystems (system: (pkgsFor system).nixfmt-rfc-style);

      packages = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          commonTools = with pkgs; [
            actionlint
            bash
            git
            jq
            nix
            nixfmt-rfc-style
            shellcheck
            shfmt
          ];
          profilePackages = import ./.nix/profile-packages.nix { inherit pkgs; };
          agentCheck = pkgs.writeShellApplication {
            name = "agent-check";
            runtimeInputs = commonTools ++ profilePackages;
            text = builtins.readFile ./.nix/agent-check.sh;
          };
        in
        {
          default = agentCheck;
          "agent-check" = agentCheck;
        }
      );

      apps = forAllSystems (system: {
        default = self.apps.${system}."agent-check";
        "agent-check" = {
          type = "app";
          program = "${self.packages.${system}."agent-check"}/bin/agent-check";
        };
      });

      checks = forAllSystems (system: {
        agentCheck = self.packages.${system}."agent-check";
      });

      devShells = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          commonTools = with pkgs; [
            actionlint
            bash
            git
            jq
            nix
            nixfmt-rfc-style
            shellcheck
            shfmt
          ];
          profilePackages = import ./.nix/profile-packages.nix { inherit pkgs; };
        in
        {
          default = import ./.nix/dev-shell.nix {
            inherit commonTools pkgs profilePackages;
            agentCheck = self.packages.${system}."agent-check";
          };
        }
      );
    };
}
NIX

{
  printf '{ pkgs }:\n'
  printf 'with pkgs; [\n'
  jq -r --arg profile "$profile" '.profiles[$profile].packages[] | "  " + .' "$profiles_file"
  printf ']\n'
} >"$output_dir/.nix/profile-packages.nix"

cat >"$output_dir/.nix/dev-shell.nix" <<'NIX'
{
  pkgs,
  agentCheck,
  commonTools,
  profilePackages,
}:
pkgs.mkShell {
  packages = commonTools ++ profilePackages ++ [ agentCheck ];

  LANG = if pkgs.stdenv.hostPlatform.isDarwin then "en_US.UTF-8" else "C.UTF-8";
  LC_ALL = if pkgs.stdenv.hostPlatform.isDarwin then "en_US.UTF-8" else "C.UTF-8";

  shellHook = ''
    repo_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
    cache_root="''${NIX_AGENT_CACHE_ROOT:-$repo_root/.cache/nix-agent}"

    export NIX_AGENT_CACHE_ROOT="$cache_root"
    export XDG_CACHE_HOME="''${XDG_CACHE_HOME:-$cache_root/xdg}"
    export CARGO_HOME="''${CARGO_HOME:-$cache_root/cargo-home}"
    export CARGO_TARGET_DIR="''${CARGO_TARGET_DIR:-$cache_root/cargo-target}"
    export GRADLE_USER_HOME="''${GRADLE_USER_HOME:-$cache_root/gradle}"
    export PUB_CACHE="''${PUB_CACHE:-$cache_root/pub}"
    export PIP_CACHE_DIR="''${PIP_CACHE_DIR:-$cache_root/pip}"
    export PNPM_HOME="''${PNPM_HOME:-$cache_root/pnpm-home}"

    mkdir -p \
      "$XDG_CACHE_HOME" \
      "$CARGO_HOME" \
      "$CARGO_TARGET_DIR" \
      "$GRADLE_USER_HOME" \
      "$PUB_CACHE" \
      "$PIP_CACHE_DIR" \
      "$PNPM_HOME"

    if [ "''${CI:-}" = "1" ] || [ "''${CI:-}" = "true" ]; then
      export NO_COLOR="''${NO_COLOR:-1}"
    fi
  '';
}
NIX

{
  cat <<SCRIPT
#!/usr/bin/env bash
set -euo pipefail

# jq variables are deliberately passed with --arg and referenced inside
# single-quoted jq programs; optional local hooks may not exist yet.
# shellcheck disable=SC1091,SC2016

# Generated profile: $profile ($description)
# Re-render from the central profile only after reviewing repository-specific
# native dependencies and commands. Keep custom work in agent-check.local.sh.

export CI="\${CI:-1}"
export NO_COLOR="\${NO_COLOR:-1}"
profile_requires_local_hook="$requires_local_hook"

repo_root="\$(git rev-parse --show-toplevel)"
cd "\$repo_root"

cache_root="\${NIX_AGENT_CACHE_ROOT:-\$repo_root/.cache/nix-agent}"
export XDG_CACHE_HOME="\${XDG_CACHE_HOME:-\$cache_root/xdg}"
export CARGO_HOME="\${CARGO_HOME:-\$cache_root/cargo-home}"
export CARGO_TARGET_DIR="\${CARGO_TARGET_DIR:-\$cache_root/cargo-target}"
export GRADLE_USER_HOME="\${GRADLE_USER_HOME:-\$cache_root/gradle}"
export PUB_CACHE="\${PUB_CACHE:-\$cache_root/pub}"
export PIP_CACHE_DIR="\${PIP_CACHE_DIR:-\$cache_root/pip}"
export PNPM_HOME="\${PNPM_HOME:-\$cache_root/pnpm-home}"
mkdir -p "\$XDG_CACHE_HOME" "\$CARGO_HOME" "\$CARGO_TARGET_DIR" "\$GRADLE_USER_HOME" "\$PUB_CACHE" "\$PIP_CACHE_DIR" "\$PNPM_HOME"

run_package_script_if_present() {
  local script_name="\$1"
  if [[ ! -f package.json ]]; then
    printf 'package.json is absent; skipping npm script %s\n' "\$script_name"
    return 0
  fi
  if jq -e --arg name "\$script_name" '.scripts[\$name] != null' package.json >/dev/null; then
    pnpm run "\$script_name"
  else
    printf 'package script %s is absent; skipping\n' "\$script_name"
  fi
}

run_local_hook() {
  if [[ -x .nix/agent-check.local.sh ]]; then
    .nix/agent-check.local.sh
    return
  fi

  if [[ "\$profile_requires_local_hook" == "true" ]]; then
    printf '%s\n' 'this profile requires an executable .nix/agent-check.local.sh with repository-specific validation' >&2
    return 78
  fi

  printf '%s\n' 'no repository-specific agent-check.local.sh; optional local stage skipped'
}

run_stage() {
  local stage="\$1"

  printf '\n==> agent-check stage: %s\n' "\$stage"
  case "\$stage" in
    preflight)
      git diff --check
      nixfmt --check flake.nix .nix/profile-packages.nix .nix/dev-shell.nix
      shellcheck .nix/agent-check.sh
      if [[ -e .nix/agent-check.local.sh ]]; then
        shellcheck .nix/agent-check.local.sh
      fi
      shfmt -i 2 -ci -d .nix/agent-check.sh
      if [[ -e .nix/agent-check.local.sh ]]; then
        shfmt -i 2 -ci -d .nix/agent-check.local.sh
      fi
      actionlint .github/workflows/nix.yml
      nix flake check --show-trace
      ;;
    profile)
SCRIPT
  jq -r --arg profile "$profile" '.profiles[$profile].commands[] | "      " + .' "$profiles_file"
  cat <<'SCRIPT'
      ;;
    local)
      run_local_hook
      ;;
    *)
      printf 'unknown agent-check stage: %s\n' "$stage" >&2
      return 64
      ;;
  esac
}

case "${1:-all}" in
  all)
    for stage in preflight profile local; do
      run_stage "$stage"
    done
    ;;
  preflight | profile | local)
    run_stage "$1"
    ;;
  *)
    printf 'usage: %s [all|preflight|profile|local]\n' "$0" >&2
    exit 64
    ;;
esac
SCRIPT
} >"$output_dir/.nix/agent-check.sh"
chmod 0755 "$output_dir/.nix/agent-check.sh"

cat >"$output_dir/.github/workflows/nix.yml" <<'YAML'
name: nix

on:
  push:
    branches: [main]
    paths:
      - flake.nix
      - flake.lock
      - .nix/**
      - .github/workflows/nix.yml
  pull_request:
    paths:
      - flake.nix
      - flake.lock
      - .nix/**
      - .github/workflows/nix.yml
  workflow_dispatch:

permissions:
  contents: read

concurrency:
  group: nix-${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

jobs:
  check:
    runs-on: ubuntu-latest
    timeout-minutes: 45
    steps:
      - name: Check out repository
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7
        with:
          persist-credentials: false

      - name: Install Nix
        uses: cachix/install-nix-action@630ae543ea3a38a9a4166f03376c02c50f408342
        with:
          extra_nix_config: |
            experimental-features = nix-command flakes

      - name: Validate the generated contract
        run: nix develop -c agent-check preflight

      - name: Run stack-profile checks
        run: nix develop -c agent-check profile

      - name: Run repository-specific checks
        run: nix develop -c agent-check local

      - name: Prove the default agent command
        run: nix develop -c agent-check
YAML

cat >"$output_dir/.nix/README.md" <<DOC
# Agent-first Nix contract

Profile: \`$profile\` — $description

Canonical entrypoints:

\`\`\`sh
nix develop
nix develop -c agent-check
nix run .#agent-check
\`\`\`

The generated command is staged as \`preflight\`, \`profile\`, and \`local\`.
Put repository-specific native dependencies in \`.nix/profile-packages.nix\` and
repository-specific validation in an executable \`.nix/agent-check.local.sh\`.
Do not add credentials, implicit cloud profiles, prompts, global configuration
mutation, or writes outside the repository-local cache root.

The renderer isolates common Cargo, Gradle, Pub, pip, pnpm, and XDG mutable state
under \`.cache/nix-agent\`. Remove unused cache variables only after verifying
that doing so does not push writes back into a user-global directory.

## Docker / OCI

$oci_posture

Nix remains a development and validation layer until service-specific binary
closure, non-root UID/GID, CA certificates, ports, entrypoint, signals, health,
size/layers, SBOM, provenance, signatures, vulnerability findings, and deployment
compatibility have been reviewed against the existing runtime.
DOC

begin_marker="# BEGIN managed nix-agent profile"
end_marker="# END managed nix-agent profile"
if [[ ! -f "$output_dir/.gitignore" ]] || ! grep -Fq "$begin_marker" "$output_dir/.gitignore"; then
  {
    printf '\n%s\n' "$begin_marker"
    printf '.cache/nix-agent/\n.direnv/\nresult\n'
    printf '%s\n' "$end_marker"
  } >>"$output_dir/.gitignore"
fi

if ((skip_lock == 0)); then
  if ! command -v nix >/dev/null 2>&1; then
    printf 'nix is required to generate flake.lock; install Nix or use --skip-lock only for fixture tests\n' >&2
    exit 69
  fi
  (
    cd "$output_dir"
    nix flake lock
  )
else
  printf '%s\n' 'warning: flake.lock was not generated; the rendered repository is not rollout-complete' >&2
fi

printf 'rendered %s profile into %s\n' "$profile" "$output_dir"
