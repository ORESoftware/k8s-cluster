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

step 'Validate toolchain and submodule contract' bash scripts/preflight.sh --contract-only

if git submodule status --recursive | grep -Eq '^[+-]'; then
  echo 'submodules must be initialized and match recorded gitlinks' >&2
  exit 1
fi

step 'Validate interface wire invariants' \
  python3 apps/cliptown-interfaces/scripts/check-wire-contract.py
step 'Validate client package layout' \
  bash apps/cliptown-clients/scripts/validate-layout.sh
step 'Validate browser-extension privacy and tests' \
  bash -c 'cd apps/cliptown-extension && npm run check'
step 'Lint and render ClipTown GitOps chart' \
  bash -c 'cd apps/cliptown-infra && helm lint . --strict && helm template cliptown-apps . >/tmp/cliptown-rendered.yaml'

if [[ "$MODE" == '--quick' ]]; then
  cat <<'EOF'

Quick cross-repository validation passed.
Run the complete language suites with:
  bash scripts/validate-workspace.sh --full
EOF
  exit 0
fi

step 'Run complete local preflight' bash scripts/preflight.sh

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
