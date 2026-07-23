{ pkgs }:
let
  flags2env = pkgs.stdenv.mkDerivation {
    pname = "flags-2-env";
    version = "git";
    src = ../tools/flags-2-env;
    nativeBuildInputs = [ pkgs.gnumake ];
    buildPhase = "make all";
    installPhase = ''
      mkdir -p $out/bin $out/lib $out/include
      cp build/flags2env $out/bin/
      cp build/libflags2env.* $out/lib/
      cp src/parser.h $out/include/flags2env.h
    '';
  };
in
pkgs.mkShell {
  packages = with pkgs; [
    rustc cargo rustfmt clippy rust-analyzer
    pkg-config openssl postgresql_17
    git gnumake clang nodejs_22 opentofu
    kubectl kustomize docker-client cloudflared
    flags2env
  ];
  shellHook = ''
    export LD_LIBRARY_PATH="${flags2env}/lib''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    export DYLD_FALLBACK_LIBRARY_PATH="${flags2env}/lib''${DYLD_FALLBACK_LIBRARY_PATH:+:$DYLD_FALLBACK_LIBRARY_PATH}"
    export OTEL_SERVICE_NAME=drone-mngr-monorepo
    echo "drone-mngr monorepo shell — git submodule update --init --recursive"
  '';
}
