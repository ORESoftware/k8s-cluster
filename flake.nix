{
  description = "drone-mngr all-up development and deployment shell";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  outputs = { nixpkgs, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
    in {
      devShells = nixpkgs.lib.genAttrs systems (system: {
        default = import ./.nix/dev-shell.nix { pkgs = import nixpkgs { inherit system; }; };
      });
    };
}
