{ pkgs }:
let
  shellPackages = with pkgs; [
    argocd
    awscli2
    bacon
    cargo
    clippy
    curl
    dart
    erlang
    git
    gleam
    go
    jq
    just
    kubectl
    kustomize
    kubernetes-helm
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
    export AWS_PROFILE="''${AWS_PROFILE:-dd-codex}"
    export NIX_DEV_SHELL=dd-k8s-cluster
  '';
}
