{ pkgs, agentCheck }:
let
  shellPackages = with pkgs; [
    agentCheck
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
    gh
    jq
    just
    kubectl
    kustomize
    kubernetes-helm
    nodejs_22
    nixfmt
    opentofu
    pnpm_10
    postgresql_16
    python312
    rust-analyzer
    rustc
    rustfmt
    ruff
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
