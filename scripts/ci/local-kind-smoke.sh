#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CLUSTER_NAME="${KIND_CLUSTER_NAME:-ores-k8s-local}"
NODE_IMAGE="${KIND_NODE_IMAGE:-kindest/node:v1.32.8@sha256:abd489f042d2b644e2d033f5c2d900bc707798d075e8186cb65e3f1367a9d5a1}"
WORKER_COUNT="${KIND_WORKERS:-0}"
KEEP_CLUSTER="${KIND_KEEP_CLUSTER:-0}"
CLUSTER_CREATED=0

require_command() {
  command -v "$1" >/dev/null 2>&1 || {
    printf 'required command is unavailable: %s\n' "$1" >&2
    return 1
  }
}

for required_command in docker git kind kubectl python3; do
  require_command "${required_command}"
done

if [[ ! "${CLUSTER_NAME}" =~ ^[a-z0-9][a-z0-9.-]{0,62}$ ]]; then
  printf 'KIND_CLUSTER_NAME must be a lower-case DNS-like name of at most 63 characters; got %s\n' \
    "${CLUSTER_NAME}" >&2
  exit 2
fi
if [[ ! "${WORKER_COUNT}" =~ ^[0-9]+$ ]] || (( WORKER_COUNT > 2 )); then
  printf 'KIND_WORKERS must be an integer from 0 through 2; got %s\n' \
    "${WORKER_COUNT}" >&2
  exit 2
fi
if [[ "${KEEP_CLUSTER}" != '0' && "${KEEP_CLUSTER}" != '1' ]]; then
  printf 'KIND_KEEP_CLUSTER must be 0 or 1; got %s\n' "${KEEP_CLUSTER}" >&2
  exit 2
fi

if [[ -n "${KIND_EVIDENCE_DIR:-}" ]]; then
  EVIDENCE_DIR="${KIND_EVIDENCE_DIR}"
  mkdir -p "${EVIDENCE_DIR}"
else
  evidence_parent="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
  mkdir -p "${evidence_parent}"
  EVIDENCE_DIR="$(mktemp -d "${evidence_parent%/}/ores-k8s-kind-smoke.XXXXXX")"
fi
EVIDENCE_DIR="$(cd "${EVIDENCE_DIR}" && pwd -P)"
if [[ "${EVIDENCE_DIR}" == '/' ]]; then
  printf 'refusing to use / as KIND_EVIDENCE_DIR\n' >&2
  exit 2
fi

KUBECONFIG_PATH="${EVIDENCE_DIR}/kubeconfig"
export EVIDENCE_DIR KUBECONFIG="${KUBECONFIG_PATH}" NODE_IMAGE WORKER_COUNT

capture_diagnostics() {
  if [[ "${CLUSTER_CREATED}" != '1' ]]; then
    return 0
  fi

  kind export logs "${EVIDENCE_DIR}/kind-logs" --name "${CLUSTER_NAME}" \
    > "${EVIDENCE_DIR}/kind-export.log" 2>&1 || true
  kubectl cluster-info dump --all-namespaces \
    > "${EVIDENCE_DIR}/cluster-info-dump.txt" 2>&1 || true
  kubectl get nodes,pods,services --all-namespaces -o wide \
    > "${EVIDENCE_DIR}/cluster-resources.txt" 2>&1 || true
}

cleanup() {
  local status=$?
  trap - EXIT

  if [[ "${status}" -ne 0 ]]; then
    capture_diagnostics
  fi

  if [[ "${CLUSTER_CREATED}" == '1' && "${KEEP_CLUSTER}" != '1' ]]; then
    kind delete cluster --name "${CLUSTER_NAME}" \
      > "${EVIDENCE_DIR}/kind-delete.log" 2>&1 || true
  fi

  printf 'kind smoke evidence: %s\n' "${EVIDENCE_DIR}"
  if [[ "${CLUSTER_CREATED}" == '1' && "${KEEP_CLUSTER}" == '1' ]]; then
    printf 'kind cluster retained for inspection: %s (KUBECONFIG=%s)\n' \
      "${CLUSTER_NAME}" "${KUBECONFIG_PATH}"
  fi
  exit "${status}"
}
trap cleanup EXIT

cd "${ROOT_DIR}"

# Fail closed instead of mutating a cluster that may contain unrelated workloads.
if kind get clusters 2>/dev/null | grep -Fxq "${CLUSTER_NAME}"; then
  printf 'kind cluster already exists; delete it or choose another KIND_CLUSTER_NAME: %s\n' \
    "${CLUSTER_NAME}" >&2
  exit 2
fi

# docker version queries both the client and daemon and therefore proves that the
# selected Docker context is usable before kind is allowed to create nodes.
docker version > "${EVIDENCE_DIR}/docker-version.txt"
docker info > "${EVIDENCE_DIR}/docker-info.txt"
kind version > "${EVIDENCE_DIR}/kind-version.txt"
kubectl version --client=true -o yaml > "${EVIDENCE_DIR}/kubectl-client-version.yaml"
git rev-parse HEAD > "${EVIDENCE_DIR}/source-sha.txt"
git status --short > "${EVIDENCE_DIR}/source-status.txt"

python3 -m py_compile \
  tools/namespace_migration.py \
  tools/test_namespace_migration.py \
  tools/namespace_manifest.py \
  tools/test_namespace_manifest.py
python3 tools/test_namespace_migration.py \
  > "${EVIDENCE_DIR}/namespace-migration-tests.log" 2>&1
python3 tools/test_namespace_manifest.py \
  > "${EVIDENCE_DIR}/namespace-manifest-tests.log" 2>&1
python3 tools/namespace_migration.py check \
  --root . \
  --format json \
  > "${EVIDENCE_DIR}/namespace-contract-report.json"
python3 tools/namespace_migration.py inventory \
  --root . \
  --format json \
  > "${EVIDENCE_DIR}/namespace-inventory.generated.json"
python3 tools/namespace_manifest.py check \
  --root . \
  --format json \
  > "${EVIDENCE_DIR}/namespace-manifest-report.json"

python3 - <<'PY_INVENTORY'
import json
import os
from pathlib import Path

root = Path.cwd()
evidence = Path(os.environ["EVIDENCE_DIR"])
contract_report = json.loads((evidence / "namespace-contract-report.json").read_text(encoding="utf-8"))
manifest_report = json.loads((evidence / "namespace-manifest-report.json").read_text(encoding="utf-8"))
committed_inventory = json.loads((root / "artifacts/namespace-inventory.json").read_text(encoding="utf-8"))
generated_inventory = json.loads((evidence / "namespace-inventory.generated.json").read_text(encoding="utf-8"))
manifest = json.loads((root / "catalog/namespaces/migration-manifest.json").read_text(encoding="utf-8"))

if contract_report.get("valid") is not True:
    raise SystemExit("namespace ownership contract is invalid")
if manifest_report.get("valid") is not True:
    raise SystemExit("namespace migration manifest contract is invalid")
if committed_inventory != generated_inventory:
    raise SystemExit("committed namespace inventory is stale for this exact source tree")
if committed_inventory.get("diagnostics") != []:
    raise SystemExit("namespace inventory contains diagnostics")

occurrences = committed_inventory.get("occurrences")
entries = manifest.get("spec", {}).get("entries")
entry_count = manifest.get("metadata", {}).get("entryCount")
if not isinstance(occurrences, list) or not isinstance(entries, list):
    raise SystemExit("namespace inventory or manifest entries are malformed")
if len(occurrences) != len(entries) or len(entries) != entry_count:
    raise SystemExit(
        "namespace inventory and manifest cardinality differ: "
        f"inventory={len(occurrences)} manifest={len(entries)} metadata={entry_count}"
    )

active_unclassified = sum(
    1
    for entry in entries
    if entry.get("scope") == "active" and entry.get("classificationStatus") == "unclassified"
)
destructive_cleanup = sum(1 for entry in entries if entry.get("destructiveCleanupAllowed") is True)
(evidence / "migration-counts.json").write_text(
    json.dumps(
        {
            "entryCount": entry_count,
            "activeUnclassifiedEntryCount": active_unclassified,
            "destructiveCleanupAuthorizedEntryCount": destructive_cleanup,
        },
        indent=2,
        sort_keys=True,
    )
    + "\n",
    encoding="utf-8",
)
PY_INVENTORY

kind_config="${EVIDENCE_DIR}/kind-config.yaml"
{
  printf '%s\n' 'kind: Cluster'
  printf '%s\n' 'apiVersion: kind.x-k8s.io/v1alpha4'
  printf '%s\n' 'nodes:'
  printf '%s\n' '  - role: control-plane'
  for ((index = 0; index < WORKER_COUNT; index += 1)); do
    printf '%s\n' '  - role: worker'
  done
} > "${kind_config}"

CLUSTER_CREATED=1
kind create cluster \
  --name "${CLUSTER_NAME}" \
  --image "${NODE_IMAGE}" \
  --config "${kind_config}" \
  --kubeconfig "${KUBECONFIG_PATH}" \
  --wait 180s

expected_context="kind-${CLUSTER_NAME}"
test "$(kubectl config current-context)" = "${expected_context}"
kubectl wait --for=condition=Ready nodes --all --timeout=180s
kubectl cluster-info > "${EVIDENCE_DIR}/cluster-info.txt"
kubectl get nodes -o wide > "${EVIDENCE_DIR}/nodes.txt"
kubectl version -o yaml > "${EVIDENCE_DIR}/kubernetes-version.yaml"
kubectl get --raw='/readyz?verbose' > "${EVIDENCE_DIR}/apiserver-readyz.txt"
kubectl api-resources > "${EVIDENCE_DIR}/api-resources.txt"

kubectl apply \
  --server-side \
  --field-manager=namespace-migration-kind-smoke \
  -f remote/argocd/observability/namespace.yaml \
  > "${EVIDENCE_DIR}/observability-namespace-apply.txt"

cat <<'YAML_CONFIGMAP' | kubectl apply --server-side --field-manager=namespace-migration-kind-smoke -f - >/dev/null
apiVersion: v1
kind: ConfigMap
metadata:
  name: namespace-migration-contract-smoke
  namespace: observability
  labels:
    platform.oresoftware.com/owner: ores
    platform.oresoftware.com/environment: local
  annotations:
    namespaces.oresoftware.com/contract: ownership-aware-v1
data:
  canonicalSecretRoot: ores/
  executionAuthorized: "false"
  legacyWritesAuthorized: "false"
  mode: read-only
YAML_CONFIGMAP

kubectl get configmap namespace-migration-contract-smoke \
  --namespace observability \
  --output json \
  > "${EVIDENCE_DIR}/namespace-contract-configmap.json"

manifests=(
  remote/argocd/observability/k8s-resource-exporter.rbac.yaml
  remote/argocd/observability/k8s-resource-exporter.configmap.yaml
  remote/argocd/observability/k8s-resource-exporter.service.yaml
  remote/argocd/observability/k8s-resource-exporter.deployment.yaml
)

for manifest_path in "${manifests[@]}"; do
  [[ -f "${manifest_path}" ]] || {
    printf 'required smoke manifest is missing: %s\n' "${manifest_path}" >&2
    exit 1
  }
  output_name="${manifest_path##*/}.server-dry-run.yaml"
  kubectl apply \
    --server-side \
    --dry-run=server \
    --field-manager=namespace-migration-kind-smoke \
    -f "${manifest_path}" \
    -o yaml \
    > "${EVIDENCE_DIR}/${output_name}"
done

kubectl apply \
  --server-side \
  --field-manager=namespace-migration-kind-smoke \
  -f remote/argocd/observability/k8s-resource-exporter.rbac.yaml \
  > "${EVIDENCE_DIR}/exporter-rbac-apply.txt"

exporter_identity='system:serviceaccount:observability:dd-k8s-resource-exporter'
pod_access="$(kubectl auth can-i list pods --all-namespaces --as="${exporter_identity}")"
secret_access="$(kubectl auth can-i get secrets --all-namespaces --as="${exporter_identity}" 2>/dev/null || true)"
printf 'identity=%s\nlist-pods=%s\nget-secrets=%s\n' \
  "${exporter_identity}" \
  "${pod_access}" \
  "${secret_access}" \
  > "${EVIDENCE_DIR}/exporter-rbac-can-i.txt"
test "${pod_access}" = 'yes'
test "${secret_access}" = 'no'

python3 - <<'PY_SUMMARY'
import json
import os
import subprocess
from pathlib import Path

evidence = Path(os.environ["EVIDENCE_DIR"])
contract = json.loads((evidence / "namespace-contract-report.json").read_text(encoding="utf-8"))
manifest_report = json.loads((evidence / "namespace-manifest-report.json").read_text(encoding="utf-8"))
migration_counts = json.loads((evidence / "migration-counts.json").read_text(encoding="utf-8"))
configmap = json.loads((evidence / "namespace-contract-configmap.json").read_text(encoding="utf-8"))
labels = configmap.get("metadata", {}).get("labels", {})
annotations = configmap.get("metadata", {}).get("annotations", {})
data = configmap.get("data", {})

api_contract_valid = (
    labels.get("platform.oresoftware.com/owner") == "ores"
    and labels.get("platform.oresoftware.com/environment") == "local"
    and annotations.get("namespaces.oresoftware.com/contract") == "ownership-aware-v1"
    and data
    == {
        "canonicalSecretRoot": "ores/",
        "executionAuthorized": "false",
        "legacyWritesAuthorized": "false",
        "mode": "read-only",
    }
)

summary = {
    "ok": True,
    "sourceSha": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
    "kindNodeImage": os.environ["NODE_IMAGE"],
    "workerCount": int(os.environ["WORKER_COUNT"]),
    "namespaceContractValid": contract.get("valid") is True,
    "namespaceManifestValid": manifest_report.get("valid") is True,
    "committedInventoryMatchesExactTree": True,
    "kubernetesApiContractValid": api_contract_valid,
    "exporterRbacLeastPrivilegeValid": True,
    "migrationCounts": migration_counts,
    "serverDryRunManifests": [
        "remote/argocd/observability/k8s-resource-exporter.rbac.yaml",
        "remote/argocd/observability/k8s-resource-exporter.configmap.yaml",
        "remote/argocd/observability/k8s-resource-exporter.service.yaml",
        "remote/argocd/observability/k8s-resource-exporter.deployment.yaml",
    ],
    "providerWrites": False,
    "secretInputs": False,
}

failed = [
    name
    for name in (
        "namespaceContractValid",
        "namespaceManifestValid",
        "committedInventoryMatchesExactTree",
        "kubernetesApiContractValid",
        "exporterRbacLeastPrivilegeValid",
    )
    if summary[name] is not True
]
if failed:
    raise SystemExit("namespace smoke invariant failed: " + ", ".join(failed))

(evidence / "summary.json").write_text(
    json.dumps(summary, indent=2, sort_keys=True) + "\n",
    encoding="utf-8",
)
PY_SUMMARY

if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
  counts="$(python3 - <<'PY_COUNTS'
import json
import os
from pathlib import Path

counts = json.loads((Path(os.environ["EVIDENCE_DIR"]) / "migration-counts.json").read_text(encoding="utf-8"))
print(
    f"entries={counts['entryCount']}, "
    f"active-unclassified={counts['activeUnclassifiedEntryCount']}, "
    f"destructive-cleanup-authorized={counts['destructiveCleanupAuthorizedEntryCount']}"
)
PY_COUNTS
)"
  {
    printf '%s\n' '## Namespace migration cloud-free kind smoke'
    printf '\n'
    printf -- '- exact source: `%s`\n' "$(git rev-parse HEAD)"
    printf -- '- isolated kubeconfig: `%s`\n' "${KUBECONFIG_PATH}"
    printf -- '- node image: `%s`\n' "${NODE_IMAGE}"
    printf -- '- topology: 1 control-plane + %s worker(s)\n' "${WORKER_COUNT}"
    printf -- '- migration inventory: %s\n' "${counts}"
    printf -- '- classifier tests, manifest tests, and exact-tree inventory parity: passed\n'
    printf -- '- Kubernetes API readiness and canonical `ores/` ConfigMap round-trip: passed\n'
    printf -- '- exporter admission dry-runs and least-privilege RBAC checks: passed\n'
    printf -- '- provider writes and secret inputs: none\n'
  } >> "${GITHUB_STEP_SUMMARY}"
fi

printf 'local kind namespace smoke passed for source %s\n' "$(git rev-parse HEAD)"
