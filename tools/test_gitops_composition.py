#!/usr/bin/env python3
"""Unit tests for the DEN-2724 GitOps composition contract."""

from __future__ import annotations

import copy
import json
import subprocess
import tempfile
import unittest
from pathlib import Path

from gitops_composition import (
    load_gitmodules,
    load_records,
    normalize_repo_url,
    render_records,
    tracked_gitlinks,
    validate_records,
)

PIN = "32be546f5ee020c1de3b099a47e6760d00e3f6e4"


def record(*, name: str = "dd-fabrication-server") -> dict:
    return {
        "$schema": "../application.schema.json",
        "apiVersion": "oresoftware.dev/v1alpha1",
        "kind": "GitOpsApplication",
        "metadata": {"name": name},
        "spec": {
            "owner": "daedalus-fab",
            "inventory": {
                "mode": "git-submodule",
                "path": "remote/deployments/fabrication-server-rs",
                "repository": "git@github.com:daedalus-fab/fabrication-server.rs.git",
                "revision": PIN,
            },
            "source": {
                "mode": "direct-repository",
                "repository": "https://github.com/daedalus-fab/fabrication-server.rs",
                "targetRevision": PIN,
                "path": "k8s",
                "renderer": "kustomize",
            },
            "argo": {
                "project": "daedalus",
                "namespace": "daedalus",
                "destinationServer": "https://kubernetes.default.svc",
                "automated": False,
                "prune": False,
                "selfHeal": False,
            },
            "migration": {
                "phase": "pilot-inert",
                "staticApplication": "remote/argocd/apps/daedalus.applications.yaml",
            },
        },
    }


class GitOpsCompositionTests(unittest.TestCase):
    def setUp(self) -> None:
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        (self.root / "catalog/gitops/apps").mkdir(parents=True)
        (self.root / "remote/argocd/apps").mkdir(parents=True)
        (self.root / "remote/argocd/apps/daedalus.applications.yaml").write_text(
            "kind: Application\n",
            encoding="utf-8",
        )
        (self.root / ".gitmodules").write_text(
            '[submodule "remote/deployments/fabrication-server-rs"]\n'
            "\tpath = remote/deployments/fabrication-server-rs\n"
            "\turl = git@github.com:daedalus-fab/fabrication-server.rs.git\n",
            encoding="utf-8",
        )
        subprocess.run(
            ["git", "init", "-q"],
            cwd=self.root,
            check=True,
        )
        subprocess.run(
            [
                "git",
                "update-index",
                "--add",
                "--cacheinfo",
                f"160000,{PIN},remote/deployments/fabrication-server-rs",
            ],
            cwd=self.root,
            check=True,
        )

    def tearDown(self) -> None:
        self.directory.cleanup()

    def write_record(self, value: dict, *, filename: str | None = None) -> Path:
        name = filename or value["metadata"]["name"]
        path = self.root / f"catalog/gitops/apps/{name}.json"
        path.write_text(
            json.dumps(value, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        return path

    def validate(self, value: dict):
        self.write_record(value)
        return validate_records(
            load_records(self.root, "catalog/gitops/apps/*.json"),
            root=self.root,
            gitmodules=load_gitmodules(self.root),
            gitlinks=tracked_gitlinks(self.root),
        )

    def test_normalize_repo_url_accepts_supported_github_forms(self):
        values = {
            normalize_repo_url("git@github.com:Daedalus-Fab/fabrication-server.rs.git"),
            normalize_repo_url(
                "ssh://git@github.com/daedalus-fab/fabrication-server.rs.git"
            ),
            normalize_repo_url(
                "https://github.com/daedalus-fab/fabrication-server.rs"
            ),
        }
        self.assertEqual(
            {"github.com/daedalus-fab/fabrication-server.rs"},
            values,
        )

    def test_valid_record_matches_gitmodules_and_index_gitlink(self):
        report = self.validate(record())
        self.assertTrue(report.valid, [item.message for item in report.diagnostics])
        self.assertEqual(1, report.records)
        self.assertEqual(0, report.errors)

    def test_target_revision_must_equal_inventory_pin(self):
        broken = record()
        broken["spec"]["source"]["targetRevision"] = "a" * 40
        report = self.validate(broken)
        rules = {item.rule_id for item in report.diagnostics}
        self.assertIn("source.pin-drift", rules)
        self.assertFalse(report.valid)

    def test_catalog_pin_must_equal_index_gitlink(self):
        broken = record()
        broken["spec"]["inventory"]["revision"] = "b" * 40
        broken["spec"]["source"]["targetRevision"] = "b" * 40
        report = self.validate(broken)
        rules = {item.rule_id for item in report.diagnostics}
        self.assertIn("inventory.gitlink-drift", rules)

    def test_infra_repository_cannot_be_an_application(self):
        broken = record(name="daedalus-infra")
        broken["spec"]["inventory"]["repository"] = (
            "git@github.com:daedalus-fab/daedalus-infra.git"
        )
        broken["spec"]["source"]["repository"] = (
            "git@github.com:daedalus-fab/daedalus-infra.git"
        )
        report = self.validate(broken)
        rules = {item.rule_id for item in report.diagnostics}
        self.assertIn("policy.infra-is-not-app", rules)

    def test_pilot_must_remain_inert(self):
        broken = record()
        broken["spec"]["argo"]["automated"] = True
        report = self.validate(broken)
        rules = {item.rule_id for item in report.diagnostics}
        self.assertIn("migration.inert-sync", rules)

    def test_renderer_uses_direct_upstream_not_cluster_gitlink_path(self):
        broken = record()
        broken["spec"]["source"]["repository"] = (
            "https://github.com/ORESoftware/k8s-cluster.git"
        )
        broken["spec"]["source"]["path"] = (
            "remote/deployments/fabrication-server-rs/k8s"
        )
        report = self.validate(broken)
        rules = {item.rule_id for item in report.diagnostics}
        self.assertIn("source.cluster-repository", rules)
        self.assertIn("source.repository-drift", rules)

    def test_render_is_deterministic_and_uses_exact_revision(self):
        value = record()
        self.write_record(value)
        loaded = load_records(self.root, "catalog/gitops/apps/*.json")
        first = render_records(loaded)
        second = render_records(loaded)
        self.assertEqual(first, second)
        self.assertEqual(
            PIN,
            first[0]["spec"]["source"]["targetRevision"],
        )
        self.assertEqual(
            "catalog-pilot-dd-fabrication-server",
            first[0]["metadata"]["name"],
        )
        self.assertNotIn("syncPolicy", first[0]["spec"])

    def test_unknown_fields_fail_in_strict_mode(self):
        broken = copy.deepcopy(record())
        broken["spec"]["inventory"]["branch"] = "main"
        report = self.validate(broken)
        rules = {item.rule_id for item in report.diagnostics}
        self.assertIn("catalog.unknown-field", rules)


if __name__ == "__main__":
    unittest.main()
