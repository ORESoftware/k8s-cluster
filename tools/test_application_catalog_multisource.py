#!/usr/bin/env python3
"""Multi-source regression tests for the Argo CD Application catalog."""

from __future__ import annotations

import unittest
from typing import Any

from application_catalog import build_catalog, normalize_repo_url, validate_catalog


def multisource_application() -> dict[str, Any]:
    return {
        "document": {
            "apiVersion": "argoproj.io/v1alpha1",
            "kind": "Application",
            "metadata": {
                "name": "mixed-sources",
                "namespace": "argocd",
            },
            "spec": {
                "destination": {
                    "namespace": "workload",
                    "server": "https://kubernetes.default.svc",
                },
                "project": "workload",
                "sources": [
                    {
                        "chart": "external-operator",
                        "repoURL": "https://charts.example.invalid",
                        "targetRevision": "1.2.3",
                    },
                    {
                        "path": "remote/deployments/service/k8s",
                        "repoURL": "ssh://git@github.com/ORESoftware/k8s-cluster.git",
                        "targetRevision": "main",
                    },
                    {
                        "path": "remote/deployments/service/k8s",
                        "repoURL": "git@github.com:acme/service.git",
                        "targetRevision": "8c2c8d8",
                    },
                ],
            },
        },
        "document_index": 3,
        "manifest_path": "remote/argocd/apps/mixed-sources.yaml",
    }


class ApplicationCatalogMultiSourceTests(unittest.TestCase):
    def test_only_cluster_repository_gitlink_sources_are_policy_violations(self):
        catalog = build_catalog(
            [multisource_application()],
            gitlinks=["remote/deployments/service"],
        )

        declaration = catalog["applications"][0]["declarations"][0]
        self.assertEqual(3, len(declaration["sources"]))
        self.assertEqual(
            ["external-operator", "", ""],
            [source["chart"] for source in declaration["sources"]],
        )

        violations = catalog["policy_violations"]["gitlink_render_paths"]
        self.assertEqual(1, len(violations))
        self.assertEqual("mixed-sources", violations[0]["application"])
        self.assertEqual(1, violations[0]["source_index"])
        self.assertEqual(
            "remote/deployments/service/k8s",
            violations[0]["source_path"],
        )
        self.assertEqual([], validate_catalog(catalog))

    def test_repository_normalization_handles_case_and_terminal_git_slashes(self):
        urls = (
            "HTTP://GitHub.com/ORESoftware/k8s-cluster.git/",
            "https://github.com/oresoftware/k8s-cluster/",
            "git@github.com:ORESoftware/k8s-cluster.git/",
        )
        self.assertEqual(
            {"github.com/oresoftware/k8s-cluster"},
            {normalize_repo_url(url) for url in urls},
        )


if __name__ == "__main__":
    unittest.main()
