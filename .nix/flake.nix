# Nix flake defining the reproducible Fiducia dev shell (Rust + Node toolchains).
{
  description = "Fiducia development environment";

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
              rustc
              cargo
              rustfmt
              clippy
              rust-analyzer

              git
              direnv
              just
              bacon

              nodejs
              pnpm

              pkg-config
              openssl
              # encrypted env files — env/enc/*.env.enc, see env/README.md
              sops
              age
            ];

            shellHook = ''
              echo "Fiducia dev shell (${system})"

              # Point sops at this machine's age key. sops finds the platform
              # default on its own, but exporting it makes the path explicit in
              # error messages and keeps macOS/Linux checkouts interchangeable.
              if [ -z "''${SOPS_AGE_KEY_FILE:-}" ]; then
                for _k in "''${XDG_CONFIG_HOME:-$HOME/.config}/sops/age/keys.txt" \
                          "$HOME/Library/Application Support/sops/age/keys.txt"; do
                  if [ -f "$_k" ]; then export SOPS_AGE_KEY_FILE="$_k"; break; fi
                done
                unset _k
              fi
              if [ -z "''${SOPS_AGE_KEY_FILE:-}" ]; then
                echo "  no age key yet — run 'just env-keygen' to create one"
              fi
            '';
          };
        });
    };
}
