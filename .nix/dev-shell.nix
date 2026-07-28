{ pkgs }:
let
  shellPackages = with pkgs; [
    actionlint
    argocd
    awscli2
    bacon
    cargo
    clippy
    curl
    dart
    beamPackages.erlang
    git
    gleam
    go
    jq
    just
    kubectl
    kustomize
    kubernetes-helm
    nix
    nixfmt-rfc-style
    nodejs_22
    opentofu
    pnpm_10
    postgresql_16
    rust-analyzer
    rustc
    rustfmt
    shellcheck
    shfmt
    yq-go
  ];
in
pkgs.mkShell {
  packages = shellPackages;

  LANG = if pkgs.stdenv.hostPlatform.isDarwin then "en_US.UTF-8" else "C.UTF-8";
  LC_ALL = if pkgs.stdenv.hostPlatform.isDarwin then "en_US.UTF-8" else "C.UTF-8";

  shellHook = ''
    export NIX_DEV_SHELL=dd-k8s-cluster
  '';
}
