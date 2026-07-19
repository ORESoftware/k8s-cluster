#!/usr/bin/env bash
set -euo pipefail

# Compile the small, explicit cross-repository contract tests against the exact
# fiducia-interfaces gitlink pinned by this superproject. Individual component
# CI cannot catch a breaking generated-interface change until that component is
# updated, so promotion must validate this set as one fleet.
#
# This script only builds/tests local checked-out sources. It has no Git or
# cloud mutation commands. `public` is suitable for ordinary CI after the
# public submodules are initialized; `full` additionally checks the protected
# application consumers after a recursive authenticated checkout.

usage() {
  cat <<'EOF'
Usage: scripts/check-interface-consumers.sh <public|full|--dry-run>

public    Validate public consensus/data-plane consumers.
full      Also validate protected application consumers; requires all apps.
--dry-run Print the exact consumers and toolchains without compiling.
EOF
}

mode="${1:-}"
case "$mode" in
  public|full|--dry-run) ;;
  -h|--help|'') usage; exit 64 ;;
  *) usage; exit 64 ;;
esac

root="$(git rev-parse --show-toplevel)"
cd "$root"

# Format: Rust toolchain, application submodule. Keep the toolchain explicit:
# a host PATH can otherwise select an unrelated Homebrew/system rustc.
public_consumers=(
  '1.95.0 fiducia-brain.rs'
  '1.95.0 fiducia-node.rs'
  '1.95.0 fiducia-routing.rs'
  '1.95.0 fiducia-load-balance.rs'
)
private_consumers=(
  '1.95.0 fiducia-auth.rs'
  '1.95.0 fiducia-admin.rs'
  '1.97.0 fiducia-customer.rs'
  '1.95.0 fiducia-cli.rs'
  '1.95.0 fiducia-node-sidecar.rs'
)
consumers=("${public_consumers[@]}")
if [[ "$mode" == 'full' ]]; then
  consumers+=("${private_consumers[@]}")
fi

if [[ "$mode" == '--dry-run' ]]; then
  for consumer in "${consumers[@]}"; do
    printf 'cargo test --locked generated_interfaces_are_importable: %s\n' "$consumer"
  done
  exit 0
fi

check_consumer() {
  local toolchain="$1"
  local repo="$2"
  local directory="apps/$repo"
  local cargo_bin rustc_bin rustdoc_bin

  test -f "$directory/Cargo.toml"
  cargo_bin="$(rustup which --toolchain "$toolchain" cargo)"
  rustc_bin="$(rustup which --toolchain "$toolchain" rustc)"
  rustdoc_bin="$(rustup which --toolchain "$toolchain" rustdoc)"

  printf 'checking %s with Rust %s\n' "$repo" "$toolchain"
  (
    cd "$directory"
    RUSTC="$rustc_bin" RUSTDOC="$rustdoc_bin" "$cargo_bin" test --locked \
      generated_interfaces_are_importable
  )
}

for consumer in "${consumers[@]}"; do
  read -r toolchain repo <<<"$consumer"
  check_consumer "$toolchain" "$repo"
done
