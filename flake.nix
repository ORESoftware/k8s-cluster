{
  description = "Agent-first development environment for the k8s-cluster repository";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { self, nixpkgs, ... }:
    let
      systems = [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-darwin"
        "x86_64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      pkgsFor = system: import nixpkgs { inherit system; };
      agentCheckFor =
        pkgs:
        pkgs.writeShellApplication {
          name = "agent-check";
          runtimeInputs = with pkgs; [
            actionlint
            check-jsonschema
            gh
            git
            nix
            nixfmt
            python312
            ruff
            shellcheck
            shfmt
            yq-go
          ];
          text = builtins.readFile ./.nix/agent-check.sh;
        };
    in
    {
      formatter = forAllSystems (system: (pkgsFor system).nixfmt);

      packages = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          agentCheck = agentCheckFor pkgs;
        in
        {
          inherit agentCheck;
          default = agentCheck;
        }
      );

      apps = forAllSystems (system: {
        "agent-check" = {
          type = "app";
          program = "${self.packages.${system}.agentCheck}/bin/agent-check";
        };
        default = self.apps.${system}."agent-check";
      });

      checks = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          agentCheck = self.packages.${system}.agentCheck;
        in
        {
          inherit agentCheck;
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

      devShells = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
        in
        {
          default = import ./.nix/dev-shell.nix {
            inherit pkgs;
            agentCheck = self.packages.${system}.agentCheck;
          };
        }
      );
    };
}
