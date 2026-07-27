{ pkgs, agentCheck }:
pkgs.mkShell {
  packages =
    (with pkgs; [
      actionlint
      cacert
      git
      gnugrep
      jq
      nixfmt-rfc-style
      openssl
      pkg-config
      rust-analyzer
      rustup
      shellcheck
      shfmt
    ])
    ++ [ agentCheck ]
    ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [ pkgs.libiconv ];

  RUST_BACKTRACE = "1";
  RUSTUP_TOOLCHAIN = "1.88.0";

  shellHook = ''
    repo_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
    cache_root="''${NIX_AGENT_CACHE_ROOT:-$repo_root/.cache/nix-agent}"
    export CARGO_HOME="''${CARGO_HOME:-$cache_root/cargo-home}"
    export CARGO_INSTALL_ROOT="''${CARGO_INSTALL_ROOT:-$cache_root/cargo-tools}"
    export CARGO_TARGET_DIR="''${CARGO_TARGET_DIR:-$cache_root/target}"
    export RUSTUP_HOME="''${RUSTUP_HOME:-$cache_root/rustup}"
    export RUSTUP_TOOLCHAIN="''${RUSTUP_TOOLCHAIN:-1.88.0}"
    export XDG_CACHE_HOME="''${XDG_CACHE_HOME:-$cache_root/xdg}"
    export PATH="$CARGO_INSTALL_ROOT/bin:$PATH"
    mkdir -p \
      "$CARGO_HOME" \
      "$CARGO_INSTALL_ROOT" \
      "$CARGO_TARGET_DIR" \
      "$RUSTUP_HOME" \
      "$XDG_CACHE_HOME"

    if ! rustup toolchain list | grep -Eq '^1\.88\.0(-|$)'; then
      rustup toolchain install 1.88.0 \
        --profile minimal \
        --component clippy \
        --component rustfmt
    fi
  '';
}
