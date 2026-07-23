{ pkgs }:
pkgs.mkShell {
  packages = with pkgs; [ nodejs_22 kubectl docker-client git gnumake ];
}
