{ pkgs }:
pkgs.mkShell {
  packages = with pkgs; [ nodejs_22 git ];
}
