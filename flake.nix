{
  description = "3FA sync server agent-first development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      rust-overlay,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };
        rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        agentRuntimeInputs =
          (with pkgs; [
            actionlint
            bash
            cacert
            cargo-audit
            cmake
            coreutils
            git
            gnumake
            gnugrep
            jq
            nix
            nixfmt
            openssl
            pkg-config
            shellcheck
            shfmt
            stdenv.cc
          ])
          ++ [ rustToolchain ]
          ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [ pkgs.libiconv ];
        agentCheck = pkgs.writeShellApplication {
          name = "agent-check";
          runtimeInputs = agentRuntimeInputs;
          runtimeEnv = {
            NIX_SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
            SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
          }
          // pkgs.lib.optionalAttrs pkgs.stdenv.isDarwin {
            LIBRARY_PATH = "${pkgs.libiconv}/lib";
            NIX_LDFLAGS = "-L${pkgs.libiconv}/lib";
          };
          text = builtins.readFile ./.nix/agent-check.sh;
        };
      in
      {
        devShells.default = import ./.nix/devshell.nix {
          inherit pkgs agentCheck agentRuntimeInputs;
        };

        packages = {
          default = agentCheck;
          agent-check = agentCheck;
        };

        apps = {
          default = flake-utils.lib.mkApp { drv = agentCheck; };
          agent-check = flake-utils.lib.mkApp { drv = agentCheck; };
        };

        checks.agent-check = agentCheck;
        formatter = pkgs.nixfmt;
      }
    );
}
