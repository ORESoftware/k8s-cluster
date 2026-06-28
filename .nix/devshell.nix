{ pkgs }:

pkgs.mkShell {
  packages = with pkgs; [
    rustc
    cargo
    rustfmt
    clippy
    rust-analyzer

    bacon
    cargo-watch
    git
    just
    pkg-config
  ];

  RUST_BACKTRACE = "1";
}
