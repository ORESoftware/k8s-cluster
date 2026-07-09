{
  description = "Sonus Auris development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { nixpkgs, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      devShells = forAllSystems (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            # dart (and, if added, flutter) are unfree.
            config.allowUnfree = true;
          };
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              # Rust — sonus-auris-backend.rs, desktop.app.rs
              rustc
              cargo
              rustfmt
              clippy
              rust-analyzer

              # Dart — sonus-auris-ui.dart
              dart

              # Web — sonus-auris-site.web
              nodejs
              pnpm

              # Tooling
              git
              direnv
              just

              # Native build deps (openssl-sys, cpal, etc.)
              pkg-config
              openssl
            ];

            shellHook = ''
              echo "Sonus Auris dev shell (${system})"
            '';
          };
        });
    };
}
