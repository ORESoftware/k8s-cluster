set -euo pipefail

export CI="${CI:-1}"
export CHECKPOINT_DISABLE="${CHECKPOINT_DISABLE:-1}"
export NO_COLOR="${NO_COLOR:-1}"
export TF_IN_AUTOMATION="${TF_IN_AUTOMATION:-1}"

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

cache_root="${NIX_AGENT_CACHE_ROOT:-$repo_root/.cache/nix-agent}"
mkdir -p "$cache_root"
export XDG_CACHE_HOME="${XDG_CACHE_HOME:-$cache_root/xdg}"
export CARGO_HOME="${CARGO_HOME:-$cache_root/cargo}"
export GRADLE_USER_HOME="${GRADLE_USER_HOME:-$cache_root/gradle}"
export PUB_CACHE="${PUB_CACHE:-$cache_root/dart-pub}"
export npm_config_cache="${npm_config_cache:-$cache_root/npm}"

mkdir -p "$XDG_CACHE_HOME" "$CARGO_HOME" "$GRADLE_USER_HOME" "$PUB_CACHE" "$npm_config_cache"

git diff --check
nixfmt --check flake.nix .nix/dev-shell.nix
shellcheck .nix/agent-check.sh
shfmt -d .nix/agent-check.sh
actionlint .github/workflows/nix.yml
nix flake check --show-trace
