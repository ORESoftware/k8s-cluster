{
  description = "canonical-interfaces development environment";

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
              # The generator + its self-tests run on Node.
              nodejs

              # Toolchains for compiling/validating the generated adapters.
              rustc
              cargo
              python3
              go

              git
              direnv
              just
            ];

            shellHook = ''
              echo "canonical-interfaces dev shell (${system})"
            '';
          };
        });
    };
}
