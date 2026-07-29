{
  description = "Development environment for the k8s-cluster repo";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { nixpkgs, ... }:
    let
      systems = [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-darwin"
        "x86_64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      packagesFor = system: import nixpkgs { inherit system; };
      agentCheckFor =
        pkgs:
        pkgs.writeShellApplication {
          name = "agent-check";
          runtimeInputs = with pkgs; [
            actionlint
            gh
            nixfmt
            python312
            ruff
          ];
          text = builtins.readFile ./.nix/agent-check.sh;
        };
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = packagesFor system;
          agentCheck = agentCheckFor pkgs;
        in
        {
          inherit agentCheck;
          default = agentCheck;
        }
      );

      devShells = forAllSystems (
        system:
        let
          pkgs = packagesFor system;
          agentCheck = agentCheckFor pkgs;
        in
        {
          default = import ./.nix/dev-shell.nix { inherit pkgs agentCheck; };
        }
      );

      checks = forAllSystems (
        system:
        let
          pkgs = packagesFor system;
          agentCheck = agentCheckFor pkgs;
        in
        {
          repository-catalog =
            pkgs.runCommand "repository-catalog-check"
              {
                nativeBuildInputs = [ agentCheck ];
                src = ./.;
              }
              ''
                cp -R "$src" source
                chmod -R u+w source
                cd source
                agent-check ci
                touch "$out"
              '';
        }
      );
    };
}
