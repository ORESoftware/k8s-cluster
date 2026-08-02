{
  pkgs,
  agentCheck,
  agentRuntimeInputs,
}:
pkgs.mkShell {
  packages = agentRuntimeInputs ++ [
    agentCheck
    pkgs.rust-analyzer
  ];

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
