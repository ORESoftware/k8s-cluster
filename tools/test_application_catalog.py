#!/usr/bin/env python3
"""Unit tests for the DEN-630 Argo CD Application catalog."""

from __future__ import annotations

import copy
import tempfile
import unittest
from pathlib import Path

from application_catalog import (
    build_catalog,
    find_gitlink_render_violations,
    normalize_repo_url,
    tracked_gitlinks,
    tracked_manifest_paths,
    validate_catalog,
)


def application_document(
    name: str,
    *,
    manifest_path: str,
    document_index: int = 0,
    repo_url: str = "git@github.com:ORESoftware/k8s-cluster.git",
    source_path: str = "remote/argocd/example",
    project: str = "default",
    destination_namespace: str = "default",
) -> dict:
    return {
        "document": {
            "apiVersion": "argoproj.io/v1alpha1",
            "kind": "Application",
            "metadata": {
                "name": name,
                "namespace": "argocd",
            },
            "spec": {
                "destination": {
                    "namespace": destination_namespace,
                    "server": "https://kubernetes.default.svc",
                },
                "project": project,
                "source": {
                    "path": source_path,
                    "repoURL": repo_url,
                    "targetRevision": "main",
                },
                "syncPolicy": {
                    "automated": {
                        "prune": True,
                        "selfHeal": True,
                    },
                    "syncOptions": ["CreateNamespace=true"],
                },
            },
        },
        "document_index": document_index,
        "manifest_path": manifest_path,
    }


class ApplicationCatalogTests(unittest.TestCase):
    def test_source_archive_fallbacks_do_not_require_git_metadata(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest = root / "remote/argocd/apps/example.yaml"
            manifest.parent.mkdir(parents=True)
            manifest.write_text("kind: Application\n", encoding="utf-8")
            (root / ".gitmodules").write_text(
                '[submodule "service"]\n'
                "\tpath = remote/deployments/service\n"
                "\turl = git@github.com:acme/service.git\n",
                encoding="utf-8",
            )
            self.assertEqual(
                ["remote/argocd/apps/example.yaml"],
                tracked_manifest_paths(root),
            )
            self.assertEqual(
                ["remote/deployments/service"],
                tracked_gitlinks(root),
            )

    def test_normalize_repo_url_accepts_supported_github_forms(self):
        urls = (
            "git@github.com:ORESoftware/k8s-cluster.git",
            "ssh://git@github.com/ORESoftware/k8s-cluster.git",
            "https://github.com/ORESoftware/k8s-cluster.git",
        )
        self.assertEqual(
            {"github.com/oresoftware/k8s-cluster"},
            {normalize_repo_url(url) for url in urls},
        )

    def test_build_catalog_groups_duplicate_names_deterministically(self):
        documents = [
            application_document(
                "shared",
                manifest_path="remote/argocd/clusters/hetzner/apps.yaml",
                document_index=2,
            ),
            application_document(
                "single",
                manifest_path="remote/argocd/apps/single.yaml",
            ),
            application_document(
                "shared",
                manifest_path="remote/argocd/clusters/aws/apps.yaml",
                document_index=1,
            ),
        ]
        catalog = build_catalog(documents, gitlinks=[])
        self.assertEqual(
            ["shared", "single"],
            [application["name"] for application in catalog["applications"]],
        )
        shared = catalog["applications"][0]
        self.assertTrue(shared["duplicate_name"])
        self.assertEqual(2, shared["declaration_count"])
        self.assertEqual(
            [
                "remote/argocd/clusters/aws/apps.yaml",
                "remote/argocd/clusters/hetzner/apps.yaml",
            ],
            [declaration["manifest_path"] for declaration in shared["declarations"]],
        )
        self.assertEqual(3, catalog["summary"]["application_documents"])
        self.assertEqual(2, catalog["summary"]["applications"])
        self.assertEqual(1, catalog["summary"]["duplicate_names"])
        self.assertEqual([], validate_catalog(catalog))

    def test_gitlink_render_violation_requires_cluster_repo_and_nested_path(self):
        unsafe = build_catalog(
            [
                application_document(
                    "unsafe",
                    manifest_path="remote/argocd/apps/unsafe.yaml",
                    source_path="remote/deployments/service/k8s",
                ),
                application_document(
                    "safe-upstream",
                    manifest_path="remote/argocd/apps/safe.yaml",
                    repo_url="git@github.com:acme/service.git",
                    source_path="k8s",
                ),
            ],
            gitlinks=["remote/deployments/service"],
        )
        violations = unsafe["policy_violations"]["gitlink_render_paths"]
        self.assertEqual(1, len(violations))
        self.assertEqual("unsafe", violations[0]["application"])
        self.assertEqual(
            "remote/deployments/service",
            violations[0]["gitlink"],
        )

    def test_find_gitlink_render_violations_handles_exact_gitlink_path(self):
        catalog = build_catalog(
            [
                application_document(
                    "unsafe",
                    manifest_path="remote/argocd/apps/unsafe.yaml",
                    source_path="remote/deployments/service",
                )
            ],
            gitlinks=[],
        )
        violations = find_gitlink_render_violations(
            catalog["applications"],
            ["remote/deployments/service"],
        )
        self.assertEqual(1, len(violations))

    def test_validation_rejects_summary_and_declaration_count_drift(self):
        catalog = build_catalog(
            [
                application_document(
                    "example",
                    manifest_path="remote/argocd/apps/example.yaml",
                )
            ],
            gitlinks=[],
        )
        broken = copy.deepcopy(catalog)
        broken["applications"][0]["declaration_count"] = 2
        broken["summary"]["applications"] = 2
        errors = validate_catalog(broken)
        self.assertIn(
            "applications[0].declaration_count is inconsistent",
            errors,
        )
        self.assertIn("summary does not match application records", errors)


if __name__ == "__main__":
    unittest.main()
