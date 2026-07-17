{
  description = "3FA web app — Supabase login + TOTP enrollment (MASH: maud, axum, SeaORM, supabase, htmx)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let pkgs = nixpkgs.legacyPackages.${system}; in {
        devShells.default = import ./nix/devshell.nix { inherit pkgs; };
      });
}
