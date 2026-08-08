#!/usr/bin/env python3
from __future__ import annotations

from collections.abc import Mapping
from typing import Any

from .core import (
    MCP_RUST_TOOLCHAIN,
    RUST_TOOLCHAIN,
    _all_repository_entries,
    common_files,
    json_text,
    python_ci,
    relationship_document,
    rust_ci,
    rust_ident,
    rust_type,
    simple_cargo_lock,
    slug,
)


def infra_files(org: Mapping[str, Any], repo: Mapping[str, Any]) -> dict[str, str]:
    prefix = slug(str(org["prefix"]))
    server_name = next(
        (item["name"] for item in _all_repository_entries(org) if item["role"] == "server"),
        f"{prefix}-server.rs",
    )
    files = common_files(org, repo)
    deployment = f"""apiVersion: apps/v1
kind: Deployment
metadata:
  name: {prefix}-server
spec:
  replicas: 2
  selector:
    matchLabels:
      app.kubernetes.io/name: {prefix}-server
  template:
    metadata:
      labels:
        app.kubernetes.io/name: {prefix}-server
    spec:
      automountServiceAccountToken: false
      securityContext:
        runAsNonRoot: true
        seccompProfile:
          type: RuntimeDefault
      containers:
        - name: server
          image: ghcr.io/{org['owner']}/{server_name}:0.1.0
          imagePullPolicy: IfNotPresent
          ports:
            - name: http
              containerPort: 8080
          readinessProbe:
            httpGet:
              path: /readyz
              port: http
          livenessProbe:
            httpGet:
              path: /healthz
              port: http
          resources:
            requests:
              cpu: 25m
              memory: 32Mi
            limits:
              cpu: 500m
              memory: 256Mi
          securityContext:
            allowPrivilegeEscalation: false
            capabilities:
              drop: ["ALL"]
            readOnlyRootFilesystem: true
"""
    files.update(
        {
            "README.md": f"""# {repo['name']}

Hardened deployment baseline for {org['product']}.

The Kubernetes workload runs non-root with no service-account token, drops all Linux capabilities, uses a read-only root filesystem, has explicit probes and resources, and is denied ingress by default except from labeled gateway workloads.
""",
            "k8s/base/deployment.yaml": deployment,
            "k8s/base/service.yaml": f"""apiVersion: v1
kind: Service
metadata:
  name: {prefix}-server
spec:
  selector:
    app.kubernetes.io/name: {prefix}-server
  ports:
    - name: http
      port: 80
      targetPort: http
""",
            "k8s/base/network-policy.yaml": f"""apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: {prefix}-server
spec:
  podSelector:
    matchLabels:
      app.kubernetes.io/name: {prefix}-server
  policyTypes: [Ingress, Egress]
  ingress:
    - from:
        - namespaceSelector:
            matchLabels:
              networking.oresoftware.dev/gateway: "true"
      ports:
        - protocol: TCP
          port: 8080
  egress:
    - to:
        - namespaceSelector: {{}}
      ports:
        - protocol: UDP
          port: 53
        - protocol: TCP
          port: 53
""",
            "k8s/base/kustomization.yaml": "resources:\n  - deployment.yaml\n  - service.yaml\n  - network-policy.yaml\n",
            "tests/test_hardening.py": """import pathlib
import unittest


class InfrastructureHardeningTest(unittest.TestCase):
    def test_deployment_has_required_controls(self) -> None:
        text = pathlib.Path("k8s/base/deployment.yaml").read_text(encoding="utf-8")
        for required in (
            "runAsNonRoot: true",
            "allowPrivilegeEscalation: false",
            "readOnlyRootFilesystem: true",
            "automountServiceAccountToken: false",
            'drop: ["ALL"]',
            "readinessProbe:",
            "livenessProbe:",
            "resources:",
        ):
            self.assertIn(required, text)
        self.assertNotIn(":latest", text)

    def test_network_policy_is_default_deny_with_explicit_flows(self) -> None:
        text = pathlib.Path("k8s/base/network-policy.yaml").read_text(encoding="utf-8")
        self.assertIn("policyTypes: [Ingress, Egress]", text)
        self.assertIn("port: 8080", text)
        self.assertIn("port: 53", text)


if __name__ == "__main__":
    unittest.main()
""",
            ".github/workflows/ci.yml": python_ci(),
        }
    )
    return files


def monorepo_files(org: Mapping[str, Any], repo: Mapping[str, Any]) -> dict[str, str]:
    document = relationship_document(org)
    files = common_files(org, repo)
    files.update(
        {
            "README.md": f"""# {repo['name']}

Coordination repository for {org['product']}.

This repository intentionally stores an immutable relationship manifest instead of copying component histories. Consumers can clone selected repositories with `scripts/clone-fleet.py`; no repository is embedded as an unaudited subtree.
""",
            "repository-map.json": json_text(document),
            "scripts/clone-fleet.py": """#!/usr/bin/env python3
import argparse
import json
import pathlib
import subprocess


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--destination", type=pathlib.Path, default=pathlib.Path("repos"))
    parser.add_argument("--role", action="append", default=[])
    parser.add_argument("--execute", action="store_true")
    args = parser.parse_args()
    document = json.loads(pathlib.Path("repository-map.json").read_text(encoding="utf-8"))
    selected = [item for item in document["repositories"] if not args.role or item["role"] in args.role]
    args.destination.mkdir(parents=True, exist_ok=True)
    for item in selected:
        target = args.destination / item["name"]
        command = ["git", "clone", "--filter=blob:none", f"https://github.com/{item['full_name']}.git", str(target)]
        print(" ".join(command))
        if args.execute:
            subprocess.run(command, check=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
""",
            "tests/test_repository_map.py": """import json
import pathlib
import unittest


class RepositoryMapTest(unittest.TestCase):
    def test_graph_has_no_self_edges_or_duplicates(self) -> None:
        document = json.loads(pathlib.Path("repository-map.json").read_text(encoding="utf-8"))
        repositories = document["repositories"]
        names = {item["full_name"] for item in repositories}
        self.assertEqual(len(names), len(repositories))
        for item in repositories:
            self.assertNotIn(item["full_name"], item["depends_on"])
            self.assertTrue(set(item["depends_on"]).issubset(names))
            self.assertTrue(set(item["used_by"]).issubset(names))


if __name__ == "__main__":
    unittest.main()
""",
            ".github/workflows/ci.yml": python_ci(),
        }
    )
    return files
