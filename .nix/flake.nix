{
  description = "shared-auth-server Rust/Postgres development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flags2env-src = {
      url = "github:oresoftware/flags-2-env";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, flags2env-src, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      eachSystem = nixpkgs.lib.genAttrs systems;
      pkgsFor = system: import nixpkgs { inherit system; config.allowUnfree = true; };
    in {
      packages = eachSystem (system:
        let pkgs = pkgsFor system;
        in {
          flags2env = pkgs.stdenv.mkDerivation {
            pname = "flags2env";
            version = "git";
            src = flags2env-src;
            nativeBuildInputs = [ pkgs.gnumake pkgs.stdenv.cc ];
            buildPhase = "make all";
            installPhase = ''
              mkdir -p $out/bin $out/lib
              cp build/flags2env $out/bin/
              cp build/libflags2env.* $out/lib/
            '';
            meta.mainProgram = "flags2env";
          };
          default = self.packages.${system}.flags2env;
        });

      devShells = eachSystem (system:
        let pkgs = pkgsFor system;
        in {
          default = pkgs.mkShell {
            packages = (with pkgs; [ rustc cargo rustfmt clippy rust-analyzer pkg-config openssl postgresql_17 redis git jq ]) ++ [ self.packages.${system}.flags2env ];
            RUST_BACKTRACE = "1";
            shellHook = ''
              echo "shared-auth-server Rust/Postgres development environment"
              echo "flags2env config: .cli-flags.toml"
            '';
          };
        });
    };
}

