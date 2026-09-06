#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

for tool in docker kind kubectl python3; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    printf 'required tool is unavailable: %s\n' "$tool" >&2
    exit 1
  fi
done

docker info >/dev/null

cluster_name="${KIND_CLUSTER_NAME:-ores-k8s-local}"
node_image="${KIND_NODE_IMAGE:-kindest/node:v1.32.8@sha256:abd489f042d2b644e2d033f5c2d900bc707798d075e8186cb65e3f1367a9d5a1}"

if kind get clusters | grep -Fxq "$cluster_name"; then
  printf 'reusing kind cluster: %s\n' "$cluster_name"
else
  kind create cluster \
    --name "$cluster_name" \
    --image "$node_image" \
    --wait 180s
fi

kubectl config use-context "kind-${cluster_name}" >/dev/null
kubectl wait --for=condition=Ready nodes --all --timeout=180s

python3 tools/test_namespace_migration.py
python3 tools/test_namespace_manifest.py

evidence_dir="$(mktemp -d)"
python3 tools/namespace_migration.py check \
  --root . \
  --format json \
  > "$evidence_dir/namespace-contract-report.json"
python3 tools/namespace_migration.py inventory \
  --root . \
  --format json \
  > "$evidence_dir/namespace-inventory.json"
python3 tools/namespace_manifest.py check \
  --root . \
  --format json \
  > "$evidence_dir/namespace-manifest-report.json"

EVIDENCE_DIR="$evidence_dir" python3 - <<'PY'
import json
import os
from pathlib import Path

root = Path(os.environ["EVIDENCE_DIR"])
contract = json.loads((root / "namespace-contract-report.json").read_text(encoding="utf-8"))
manifest = json.loads((root / "namespace-manifest-report.json").read_text(encoding="utf-8"))

if contract.get("valid") is not True:
    raise SystemExit("namespace ownership contract is invalid")
if manifest.get("valid") is not True:
    raise SystemExit("namespace migration manifest is invalid")
if manifest.get("entryCount") != manifest.get("inventoryEntryCount"):
    raise SystemExit("namespace inventory and manifest counts differ")
if manifest.get("diagnostics") != []:
    raise SystemExit("namespace migration manifest has diagnostics")

print(
    "namespace contract valid; "
    f"entries={manifest.get('entryCount')} "
    f"unclassified={manifest.get('unclassifiedEntryCount', 'reported-in-manifest')}"
)
PY

kubectl apply --server-side -f remote/argocd/observability/namespace.yaml >/dev/null

for manifest in \
  remote/argocd/observability/k8s-resource-exporter.rbac.yaml \
  remote/argocd/observability/k8s-resource-exporter.configmap.yaml \
  remote/argocd/observability/k8s-resource-exporter.service.yaml \
  remote/argocd/observability/k8s-resource-exporter.deployment.yaml
do
  kubectl apply --server-side --dry-run=server -f "$manifest" >/dev/null
done

kubectl apply --server-side -f remote/argocd/observability/k8s-resource-exporter.rbac.yaml >/dev/null
test "$(kubectl auth can-i list pods \
  --as=system:serviceaccount:observability:dd-k8s-resource-exporter)" = "yes"

cat <<'YAML' | kubectl apply --server-side -f - >/dev/null
apiVersion: v1
kind: ConfigMap
metadata:
  name: namespace-contract-smoke
  namespace: observability
  labels:
    app.kubernetes.io/managed-by: k8s-cluster
    app.kubernetes.io/part-of: ores-platform
  annotations:
    namespaces.oresoftware.com/contract: ownership-aware-v1
data:
  owner: ores
  mode: read-only
  executionAuthorized: "false"
YAML

python3 - <<'PY'
import json
import subprocess

payload = json.loads(
    subprocess.check_output(
        [
            "kubectl",
            "get",
            "configmap",
            "namespace-contract-smoke",
            "--namespace",
            "observability",
            "--output",
            "json",
        ],
        text=True,
    )
)

assert payload["metadata"]["labels"]["app.kubernetes.io/part-of"] == "ores-platform"
assert payload["metadata"]["annotations"]["namespaces.oresoftware.com/contract"] == "ownership-aware-v1"
assert payload["data"] == {
    "executionAuthorized": "false",
    "mode": "read-only",
    "owner": "ores",
}
PY

kubectl get nodes -o wide
kubectl get namespace observability
printf 'local kind smoke passed; cluster retained for inspection: %s\n' "$cluster_name"
