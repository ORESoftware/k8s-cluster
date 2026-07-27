{ pkgs, agentCheck }:
pkgs.mkShell {
  packages =
    (with pkgs; [
      actionlint
      cargo
      cargo-audit
      clippy
      git
      jq
      nixfmt-rfc-style
      openssl
      pkg-config
      rust-analyzer
      rustc
      rustfmt
      shellcheck
      shfmt
    ])
    ++ [ agentCheck ]
    ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [ pkgs.libiconv ];

  RUST_BACKTRACE = "1";

  shellHook = ''
    repo_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
    cache_root="''${NIX_AGENT_CACHE_ROOT:-$repo_root/.cache/nix-agent}"
    export CARGO_HOME="''${CARGO_HOME:-$cache_root/cargo-home}"
    export CARGO_TARGET_DIR="''${CARGO_TARGET_DIR:-$cache_root/target}"
    export XDG_CACHE_HOME="''${XDG_CACHE_HOME:-$cache_root/xdg}"
    mkdir -p "$CARGO_HOME" "$CARGO_TARGET_DIR" "$XDG_CACHE_HOME"
  '';
}
