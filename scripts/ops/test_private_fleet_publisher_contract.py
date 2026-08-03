#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
WORKFLOW_PATH = (
    ROOT / ".github/workflows/ops-publish-missing-org-repositories-gh-profile.yml"
)
PUBLISHER_PATH = ROOT / "scripts/ops/publish_missing_org_repositories_current.py"


class PrivateFleetPublisherContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
        cls.publisher = PUBLISHER_PATH.read_text(encoding="utf-8")

    def test_workflow_checks_out_and_binds_the_exact_main_event_sha(self) -> None:
        self.assertIn("ref: ${{ github.sha }}", self.workflow)
        self.assertNotIn("ref: main\n          fetch-depth", self.workflow)
        self.assertIn('test "$GITHUB_REF" = refs/heads/main', self.workflow)
        self.assertIn(
            'test "$(git rev-parse HEAD)" = "$GITHUB_SHA"', self.workflow
        )

    def test_projection_tests_run_before_any_repository_publication(self) -> None:
        test_command = (
            'python3 "$work/k8s-cluster/scripts/ops/'
            'test_repository_fleet_visibility.py" -v'
        )
        test_index = self.workflow.index(test_command)
        publication_index = self.workflow.index(
            "stage=bounded-repository-publication"
        )
        self.assertLess(test_index, publication_index)

    def test_remote_process_rejects_ambient_credentials(self) -> None:
        inherited_guard = (
            "CODEX_HOME GH_TOKEN GITHUB_TOKEN "
            "GITHUB_REPOSITORY_ADMIN_TOKEN GIT_ASKPASS"
        )
        self.assertIn(inherited_guard, self.workflow)
        self.assertIn("GIT_ASKPASS_REQUIRE=force", self.workflow)
        self.assertIn("GIT_TERMINAL_PROMPT=0", self.workflow)
        self.assertIn("GIT_CONFIG_KEY_0=credential.helper", self.workflow)

    def test_sealed_public_ledger_is_checked_before_private_projection(self) -> None:
        public_check = self.publisher.index(
            'record.get("visibility") != "public"'
        )
        projection = self.publisher.index(
            "execution_manifest = project_private_execution_manifest"
        )
        self.assertLess(public_check, projection)

    def test_only_private_execution_manifest_reaches_execute_loop(self) -> None:
        projection = self.publisher.index(
            "execution_manifest = project_private_execution_manifest"
        )
        execute = self.publisher.index('"--execute"')
        self.assertLess(projection, execute)
        self.assertIn('str(execution_manifest_path)', self.publisher)
        self.assertNotIn(
            '"--manifest",\n                str(generated_manifest_path)',
            self.publisher,
        )

    def test_missing_repositories_are_created_private_without_visibility_patch(self) -> None:
        self.assertIn('"private": True', self.publisher)
        self.assertIn('current.get("private") is not True', self.publisher)
        self.assertIn('current.get("visibility") != "private"', self.publisher)
        self.assertNotIn('MODULE.api("PATCH"', self.publisher)

    def test_publication_has_no_force_path(self) -> None:
        self.assertNotIn('"--force"', self.publisher)
        self.assertNotIn("git push --force", self.workflow)
        self.assertNotIn("git push -f", self.workflow)

    def test_remote_main_sha_and_private_visibility_are_both_verified(self) -> None:
        sha_check = self.publisher.index("if actual != expected:")
        metadata_fetch = self.publisher.index(
            'status, remote = MODULE.api("GET", f"/repos/{full_name}")'
        )
        private_check = self.publisher.index(
            'remote.get("private") is not True'
        )
        visibility_check = self.publisher.index(
            'remote.get("visibility") != "private"'
        )
        self.assertLess(sha_check, metadata_fetch)
        self.assertLess(metadata_fetch, private_check)
        self.assertLess(metadata_fetch, visibility_check)
        self.assertIn("VERIFIED 32/32 private", self.publisher)


if __name__ == "__main__":
    unittest.main()
