{ pkgs, agentCheck }:
let
  shellPackages =
    (with pkgs; [
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
      nix
      nixfmt
      nodejs_22
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
    ])
    ++ [ agentCheck ];
in
pkgs.mkShell {
  packages = shellPackages;

  LANG = if pkgs.stdenv.hostPlatform.isDarwin then "en_US.UTF-8" else "C.UTF-8";
  LC_ALL = if pkgs.stdenv.hostPlatform.isDarwin then "en_US.UTF-8" else "C.UTF-8";

  shellHook = ''
    export NIX_DEV_SHELL=dd-k8s-cluster
    export NIX_AGENT_CACHE_ROOT="''${NIX_AGENT_CACHE_ROOT:-$PWD/.cache/nix-agent}"
    mkdir -p "$NIX_AGENT_CACHE_ROOT"

    # Credentials and cloud identities are deliberately not selected here.
    # Agents and operators must opt in with explicit environment variables.
    if [ "''${CI:-}" = "1" ] || [ "''${CI:-}" = "true" ]; then
      export CHECKPOINT_DISABLE="''${CHECKPOINT_DISABLE:-1}"
      export NO_COLOR="''${NO_COLOR:-1}"
      export TF_IN_AUTOMATION="''${TF_IN_AUTOMATION:-1}"
    fi
  '';
}
