{
  description = "akrion-web-server.rs development environment";

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
              bacon

              nodejs
              chromium

              git
              direnv
              just
              pkg-config
              openssl
            ];

            shellHook = ''
              export PUPPETEER_SKIP_DOWNLOAD=1
              export PUPPETEER_EXECUTABLE_PATH="${pkgs.chromium}/bin/chromium"
              export PLAYWRIGHT_CHROMIUM="${pkgs.chromium}/bin/chromium"
              echo "akrion-web-server dev shell (${system})"
            '';
          };
        });
    };
}
