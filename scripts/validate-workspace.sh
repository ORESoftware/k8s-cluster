#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT_DIR"

MODE=${1:---quick}
if [[ "$MODE" != '--quick' && "$MODE" != '--full' ]]; then
  echo 'usage: bash scripts/validate-workspace.sh [--quick|--full]' >&2
  exit 2
fi

step() {
  local name=$1
  shift
  printf '\n==> %s\n' "$name"
  "$@"
}

quick_step() {
  local capability_path=$1
  local name=$2
  shift 2
  if [[ -e "$capability_path" ]]; then
    step "$name" "$@"
  else
    printf '\n==> Skip %s\n' "$name"
    printf 'Recorded gitlink does not yet contain %s; the later pointer-rollout PR will make this check mandatory.\n' "$capability_path"
  fi
}

step 'Validate toolchain and submodule contract' bash scripts/preflight.sh --contract-only
step 'Validate MemeBank API/SDK boundary' python3 scripts/check-memebank-boundary.py

if git submodule status --recursive | grep -Eq '^[+-]'; then
  echo 'submodules must be initialized and match recorded gitlinks' >&2
  exit 1
fi

quick_step 'apps/cliptown-interfaces/scripts/check-wire-contract.py' \
  'Validate interface wire invariants' \
  bash -c 'cd apps/cliptown-interfaces && python3 scripts/check-wire-contract.py'
quick_step 'apps/cliptown-clients/scripts/validate-layout.sh' \
  'Validate client package layout' \
  bash -c 'cd apps/cliptown-clients && bash scripts/validate-layout.sh'
quick_step 'apps/cliptown-extension/package.json' \
  'Validate browser-extension privacy and tests' \
  bash -c 'cd apps/cliptown-extension && npm run check'
quick_step 'apps/cliptown-infra/Chart.yaml' \
  'Lint and render ClipTown GitOps chart' \
  bash -c 'cd apps/cliptown-infra && helm lint . --strict && helm template cliptown-apps . >/tmp/cliptown-rendered.yaml'

if [[ "$MODE" == '--quick' ]]; then
  cat <<'EOF'

Quick cross-repository validation passed for every capability present at the recorded gitlinks.
Run the complete language suites after the pointer rollout with:
  bash scripts/validate-workspace.sh --full
EOF
  exit 0
fi

step 'Run complete local preflight' bash scripts/preflight.sh

for required_path in \
  apps/cliptown-interfaces/scripts/check-wire-contract.py \
  apps/cliptown-clients/scripts/validate-layout.sh \
  apps/cliptown-extension/package.json \
  apps/cliptown-infra/Chart.yaml; do
  if [[ ! -e "$required_path" ]]; then
    echo "--full requires an updated recorded gitlink containing $required_path" >&2
    exit 1
  fi
done

if ! command -v buf >/dev/null 2>&1; then
  echo 'buf is required for --full contract validation' >&2
  exit 1
fi

step 'Lint Protobuf contracts' bash -c 'cd apps/cliptown-interfaces && buf lint'
step 'Test generated Rust interfaces' bash -c 'cargo fmt --manifest-path apps/cliptown-interfaces/generated/rust/Cargo.toml --check && cargo clippy --manifest-path apps/cliptown-interfaces/generated/rust/Cargo.toml --all-targets -- -D warnings && cargo test --manifest-path apps/cliptown-interfaces/generated/rust/Cargo.toml'
step 'Test generated TypeScript interfaces' bash -c 'cd apps/cliptown-interfaces/generated/typescript && npm install && npm run typecheck && npm run build'
step 'Analyze generated Dart interfaces' bash -c 'cd apps/cliptown-interfaces/generated/dart && dart pub get && dart analyze'

step 'Test Rust SDK' bash -c 'cd apps/cliptown-clients/clients/rust && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --all-targets'
step 'Test TypeScript SDK' bash -c 'cd apps/cliptown-clients/clients/typescript && npm install && npm run typecheck && npm test && npm run build'
step 'Test Dart SDK' bash -c 'cd apps/cliptown-clients/clients/dart && dart pub get && dart format --output=none --set-exit-if-changed lib test && dart analyze && dart test'

step 'Test Rust backend' bash -c 'cd apps/cliptown-rust-backend.rs && cargo metadata --locked --format-version 1 --no-deps >/dev/null && cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked --all-targets'
step 'Test Rust CLI' bash -c 'cd apps/cliptown-cli && cargo metadata --locked --format-version 1 --no-deps >/dev/null && cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked --all-targets'
step 'Analyze and test Flutter application' bash -c 'cd apps/cliptown-flutter && flutter pub get && dart format --output=none --set-exit-if-changed lib test integration_test && flutter analyze --fatal-infos --fatal-warnings && flutter test'

cat <<'EOF'

Full local validation passed.
Platform-only emulator, native packaging, signing, notarization, and store checks remain delegated to their hosted operating-system runners.
EOF
