#!/usr/bin/env python3
"""Execute a credential-free kind smoke and emit RuntimeEvidence.

This script is CI/runtime evidence only. It does not modify either authored
contract authority and it accepts no provider credentials or cloud mutation
inputs.
"""

from __future__ import annotations

import importlib.util
import json
import os
import platform
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PROFILE_VERIFIER_PATH = ROOT / "scripts/local-k8s/verify_profile.py"
EVIDENCE_VERIFIER_PATH = ROOT / "scripts/local-k8s/verify_evidence.py"


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


VERIFY_PROFILE = load_module("verify_profile", PROFILE_VERIFIER_PATH)
VERIFY_EVIDENCE = load_module("verify_evidence", EVIDENCE_VERIFIER_PATH)


def run(command: list[str], *, input_text: str | None = None) -> str:
    completed = subprocess.run(
        command,
        cwd=ROOT,
        input=input_text,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"command failed: {command[0]} {command[1] if len(command) > 1 else ''}".strip())
    return completed.stdout


def normalized_architecture() -> str:
    machine = platform.machine().lower()
    if machine in {"x86_64", "amd64"}:
        return "amd64"
    if machine in {"aarch64", "arm64"}:
        return "arm64"
    raise RuntimeError("unsupported host architecture for local kind evidence")


def main() -> int:
    if len(sys.argv) != 1:
        print("live_kind_evidence.py accepts no command-line arguments", file=sys.stderr)
        return 2

    profile_path = Path(
        os.environ.get(
            "LOCAL_K8S_PROFILE",
            "contracts/local-k8s-platform/instances/RuntimeProfile/valid/ci-kind.json",
        )
    )
    evidence_dir_raw = os.environ.get("LOCAL_K8S_EVIDENCE_DIR", "")
    cluster_name = os.environ.get("LOCAL_K8S_CLUSTER_NAME", "ores-den1032-ci-kind")
    keep_cluster = os.environ.get("LOCAL_K8S_KEEP_CLUSTER", "0")
    if keep_cluster not in {"0", "1"}:
        print("LOCAL_K8S_KEEP_CLUSTER must be 0 or 1", file=sys.stderr)
        return 2
    if not evidence_dir_raw:
        print("LOCAL_K8S_EVIDENCE_DIR is required", file=sys.stderr)
        return 2

    evidence_dir = Path(evidence_dir_raw)
    evidence_dir.mkdir(parents=True, exist_ok=True)
    kubeconfig = evidence_dir / "kubeconfig"
    evidence_path = evidence_dir / "runtime-evidence.json"

    try:
        profile_document = VERIFY_PROFILE.load_profile(ROOT / profile_path)
        if profile_document["runtime"] != "docker-kind":
            raise RuntimeError("live CI evidence requires the docker-kind RuntimeProfile")
        if normalized_architecture() != profile_document["architecture"]:
            raise RuntimeError("host architecture does not match RuntimeProfile architecture")

        run(["docker", "version"])
        existing = {line.strip() for line in run(["kind", "get", "clusters"]).splitlines() if line.strip()}
        if cluster_name in existing:
            raise RuntimeError("refusing to reuse an existing kind cluster")

        created = False
        try:
            run(
                [
                    "kind",
                    "create",
                    "cluster",
                    "--name",
                    cluster_name,
                    "--image",
                    profile_document["runtimeArtifact"],
                    "--kubeconfig",
                    str(kubeconfig),
                    "--wait",
                    "180s",
                ]
            )
            created = True

            kubectl = ["kubectl", "--kubeconfig", str(kubeconfig)]
            run(kubectl + ["wait", "--for=condition=Ready", "nodes", "--all", "--timeout=180s"])
            readyz = run(kubectl + ["get", "--raw=/readyz"]).strip()
            if readyz != "ok":
                raise RuntimeError("Kubernetes API /readyz did not report ok")

            version = json.loads(run(kubectl + ["version", "-o", "json"]))
            server_version = version.get("serverVersion", {}).get("gitVersion")
            if server_version != profile_document["kubernetesVersion"]:
                raise RuntimeError("Kubernetes server version does not match RuntimeProfile")

            nodes = json.loads(run(kubectl + ["get", "nodes", "-o", "json"]))
            node_count = len(nodes.get("items", []))
            expected_nodes = {"single-node": 1, "three-node": 3}[profile_document["topology"]]
            if node_count != expected_nodes:
                raise RuntimeError("kind node count does not match RuntimeProfile topology")

            namespace = "ores-local-contract"
            identity = f"system:serviceaccount:{namespace}:runtime-evidence"
            admission_manifest = f"""apiVersion: v1
kind: Namespace
metadata:
  name: {namespace}
  labels:
    platform.oresoftware.com/owner: ores
    platform.oresoftware.com/environment: local
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: runtime-evidence-contract
  namespace: {namespace}
data:
  mode: read-only
  cloudWrites: \"false\"
"""
            run(
                kubectl
                + [
                    "apply",
                    "--server-side",
                    "--dry-run=server",
                    "--field-manager=den-1032-runtime-evidence",
                    "-f",
                    "-",
                ],
                input_text=admission_manifest,
            )

            rbac_manifest = f"""apiVersion: v1
kind: Namespace
metadata:
  name: {namespace}
---
apiVersion: v1
kind: ServiceAccount
metadata:
  name: runtime-evidence
  namespace: {namespace}
---
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: runtime-evidence-pod-reader
  namespace: {namespace}
rules:
  - apiGroups: [\"\"]
    resources: [\"pods\"]
    verbs: [\"get\", \"list\", \"watch\"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: runtime-evidence-pod-reader
  namespace: {namespace}
subjects:
  - kind: ServiceAccount
    name: runtime-evidence
    namespace: {namespace}
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: Role
  name: runtime-evidence-pod-reader
"""
            run(
                kubectl
                + [
                    "apply",
                    "--server-side",
                    "--field-manager=den-1032-runtime-evidence",
                    "-f",
                    "-",
                ],
                input_text=rbac_manifest,
            )

            list_pods = run(
                kubectl
                + [
                    "auth",
                    "can-i",
                    "list",
                    "pods",
                    "--namespace",
                    namespace,
                    f"--as={identity}",
                ]
            ).strip()
            secret_probe = subprocess.run(
                kubectl
                + [
                    "auth",
                    "can-i",
                    "get",
                    "secrets",
                    "--namespace",
                    namespace,
                    f"--as={identity}",
                ],
                cwd=ROOT,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            get_secrets = secret_probe.stdout.strip()
            if list_pods != "yes" or get_secrets != "no":
                raise RuntimeError("least-privilege RBAC contract did not converge")

            source_sha = run(["git", "rev-parse", "HEAD"]).strip()
            evidence = {
                "schema": "ores.local-k8s-runtime-evidence/v1",
                "profileName": profile_document["name"],
                "profileSha256": VERIFY_EVIDENCE.profile_digest(ROOT / profile_path),
                "evidenceSource": "github-actions-kind",
                "sourceSha": source_sha,
                "runtime": profile_document["runtime"],
                "architecture": profile_document["architecture"],
                "resourceClass": profile_document["resourceClass"],
                "topology": profile_document["topology"],
                "kubernetesVersion": profile_document["kubernetesVersion"],
                "runtimeArtifact": profile_document["runtimeArtifact"],
                "apiServerReady": True,
                "serverAdmissionPassed": True,
                "rbacListPods": True,
                "rbacGetSecrets": False,
                "providerWrites": False,
                "secretInputs": False,
                "cloudMutationPolicy": profile_document["cloudMutationPolicy"],
                "cloudWrites": False,
            }
            VERIFY_EVIDENCE.validate_evidence(
                profile_document,
                evidence,
                expected_profile_sha256=VERIFY_EVIDENCE.profile_digest(ROOT / profile_path),
            )
            evidence_path.write_text(
                json.dumps(evidence, indent=2, sort_keys=True) + "\n",
                encoding="utf-8",
            )
            print(str(evidence_path))
        finally:
            if created and keep_cluster != "1":
                subprocess.run(
                    ["kind", "delete", "cluster", "--name", cluster_name],
                    cwd=ROOT,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    check=False,
                )
    except (OSError, ValueError, RuntimeError, json.JSONDecodeError) as exc:
        print(f"live kind evidence rejected: {exc}", file=sys.stderr)
        return 2

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
