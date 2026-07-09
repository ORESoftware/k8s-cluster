{
  description = "canonical-frontend development environment";

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
              nodejs
              pnpm

              git
              direnv
              just

              # Playwright/Puppeteer drive a real Chromium for the e2e specs.
              chromium
            ];

            # Point Playwright/Puppeteer at the Nix-provided Chromium so the e2e
            # specs don't need a separate browser download.
            shellHook = ''
              export PUPPETEER_SKIP_DOWNLOAD=1
              export PUPPETEER_EXECUTABLE_PATH="${pkgs.chromium}/bin/chromium"
              export PLAYWRIGHT_CHROMIUM="${pkgs.chromium}/bin/chromium"
              echo "canonical-frontend dev shell (${system})"
            '';
          };
        });
    };
}
