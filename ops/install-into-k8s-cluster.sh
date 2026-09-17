#!/usr/bin/env bash
set -euo pipefail

cluster_dir="${1:-${K8S_CLUSTER_DIR:-$HOME/codes/ores/k8s-cluster}}"
repo_url="${MIP_SOLVER_NODE_REPO_URL:-https://github.com/ORESoftware/mip-solver-node.rs.git}"
submodule_path="remote/deployments/mip-solver-node.rs"
app_path="remote/argocd/apps/dd-in-house-mip-solver-node.application.yaml"

if [ ! -d "$cluster_dir/.git" ]; then
  echo "k8s-cluster repo not found at: $cluster_dir" >&2
  exit 1
fi

cd "$cluster_dir"

if [ -d "$submodule_path/.git" ] || git config --file .gitmodules --get-regexp "submodule\..*\.path" | grep -q " $submodule_path$"; then
  git submodule update --init "$submodule_path"
  git -C "$submodule_path" fetch origin main
  git -C "$submodule_path" checkout main
  git -C "$submodule_path" pull --ff-only origin main
else
  git submodule add "$repo_url" "$submodule_path"
fi

mkdir -p "$(dirname "$app_path")"
cp "$submodule_path/ops/dd-in-house-mip-solver-node.application.yaml" "$app_path"

kubectl kustomize "$submodule_path/k8s" >/tmp/dd-in-house-mip-solver-node.rendered.yaml

echo "Installed $submodule_path"
echo "Copied $app_path"
echo "Rendered Kustomize output: /tmp/dd-in-house-mip-solver-node.rendered.yaml"
echo "Next: review git status, commit .gitmodules, $submodule_path, and $app_path."
echo "Note: the Argo CD app sources the solver repo directly; the submodule is for EC2/local dependency layout."
