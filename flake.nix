{
  description = "3FA sync server agent-first development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        agentCheck = pkgs.writeShellApplication {
          name = "agent-check";
          runtimeInputs =
            (with pkgs; [
              actionlint
              bash
              cacert
              cargo-audit
              git
              gnugrep
              nixfmt-rfc-style
              openssl
              pkg-config
              rustup
              shellcheck
              shfmt
            ])
            ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [ pkgs.libiconv ];
          text = builtins.readFile ./.nix/agent-check.sh;
        };
      in
      {
        devShells.default = import ./.nix/devshell.nix { inherit pkgs agentCheck; };

        packages = {
          default = agentCheck;
          agent-check = agentCheck;
        };

        apps = {
          default = flake-utils.lib.mkApp { drv = agentCheck; };
          agent-check = flake-utils.lib.mkApp { drv = agentCheck; };
        };

        checks.agent-check = agentCheck;
        formatter = pkgs.nixfmt-rfc-style;
      }
    );
}
