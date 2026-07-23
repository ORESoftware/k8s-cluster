{ pkgs }:
pkgs.mkShell {
  packages = with pkgs; [ gleam erlang_27 rebar3 nodejs_22 git gnumake ];
}
