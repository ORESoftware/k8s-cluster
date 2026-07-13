{
  description = "canonical-monorepo development environment";

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
          pkgs = import nixpkgs { inherit system; };
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              # Rust (canonical-web-server.rs)
              rustc
              cargo
              rustfmt
              clippy
              rust-analyzer
              bacon

              # Node (marketing/app clients + superproject contract tests)
              nodejs
              pnpm

              git
              direnv
              just

              pkg-config
              openssl
            ];

            shellHook = ''
              echo "canonical-monorepo dev shell (${system})"
            '';
          };
        });
    };
}
