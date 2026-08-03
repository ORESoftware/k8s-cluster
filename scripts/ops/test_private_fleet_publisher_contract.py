#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
WORKFLOW_PATH = (
    ROOT / ".github/workflows/ops-publish-missing-org-repositories-gh-profile.yml"
)
PUBLISHER_PATH = ROOT / "scripts/ops/publish_missing_org_repositories_current.py"
REMOTE_STATE_PATH = ROOT / "scripts/ops/repository_fleet_remote_state.py"


class PrivateFleetPublisherContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
        cls.publisher = PUBLISHER_PATH.read_text(encoding="utf-8")
        cls.remote_state = REMOTE_STATE_PATH.read_text(encoding="utf-8")

    def test_workflow_checks_out_and_binds_the_exact_main_event_sha(self) -> None:
        self.assertIn("ref: ${{ github.sha }}", self.workflow)
        self.assertNotIn("ref: main\n          fetch-depth", self.workflow)
        self.assertIn('test "$GITHUB_REF" = refs/heads/main', self.workflow)
        self.assertIn(
            'test "$(git rev-parse HEAD)" = "$GITHUB_SHA"', self.workflow
        )

    def test_negative_contract_suites_run_before_any_repository_publication(self) -> None:
        visibility_test = (
            'python3 "$work/k8s-cluster/scripts/ops/'
            'test_repository_fleet_visibility.py" -v'
        )
        remote_state_test = (
            'python3 "$work/k8s-cluster/scripts/ops/'
            'test_repository_fleet_remote_state.py" -v'
        )
        publication_index = self.workflow.index(
            "stage=bounded-repository-publication"
        )
        self.assertLess(self.workflow.index(visibility_test), publication_index)
        self.assertLess(self.workflow.index(remote_state_test), publication_index)

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

    def test_private_projection_and_remote_partition_precede_execute(self) -> None:
        projection = self.publisher.index(
            "execution_manifest = project_private_execution_manifest"
        )
        partition = self.publisher.index(
            "missing_records, existing_snapshot = classify_remote_fleet"
        )
        execute = self.publisher.index('"--execute"')
        self.assertLess(projection, partition)
        self.assertLess(partition, execute)
        self.assertIn('str(execution_manifest_path)', self.publisher)
        self.assertNotIn(
            '"--manifest",\n                str(generated_manifest_path)',
            self.publisher,
        )

    def test_only_missing_records_reach_the_live_publisher(self) -> None:
        self.assertIn("for record in missing_records:", self.publisher)
        self.assertIn("verify_created_repositories(", self.publisher)
        self.assertIn("verify_preserved_existing(", self.publisher)
        self.assertNotIn("for record in execution_records:\n        full_name", self.publisher)
        self.assertNotIn("VERIFIED 32/32 private", self.publisher)

    def test_existing_divergent_histories_are_explicitly_preserved(self) -> None:
        self.assertIn('"DIVERGENT_REVIEWED"', self.publisher)
        self.assertIn('print(f"PRESERVE_{disposition}', self.publisher)
        self.assertIn("VERIFIED_PRESERVED_PRIVATE", self.publisher)
        self.assertIn("existing repository", self.remote_state)
        self.assertIn("changed during gap publication", self.remote_state)
        self.assertIn("matches_sealed_commit", self.remote_state)

    def test_missing_repositories_are_created_private_without_visibility_patch(self) -> None:
        self.assertIn('"private": True', self.publisher)
        self.assertIn('current.get("private") is not True', self.publisher)
        self.assertIn('current.get("visibility") != "private"', self.publisher)
        self.assertNotIn('MODULE.api("PATCH"', self.publisher)

    def test_publication_has_no_force_path(self) -> None:
        self.assertNotIn('"--force"', self.publisher)
        self.assertNotIn("git push --force", self.workflow)
        self.assertNotIn("git push -f", self.workflow)

    def test_created_sha_and_preserved_head_are_distinct_final_contracts(self) -> None:
        created = self.publisher.index("verify_created_repositories(")
        preserved = self.publisher.index("verify_preserved_existing(")
        created_evidence = self.publisher.index("VERIFIED_CREATED_PRIVATE")
        preserved_evidence = self.publisher.index("VERIFIED_PRESERVED_PRIVATE")
        self.assertLess(created, created_evidence)
        self.assertLess(preserved, preserved_evidence)
        self.assertIn("created repository", self.remote_state)
        self.assertIn("main drift", self.remote_state)

    def test_missing_monorepo_is_ordered_after_leaf_repositories(self) -> None:
        self.assertIn('record.get("kind") == "monorepo"', self.remote_state)
        self.assertIn("Missing leaf histories must be published", self.remote_state)


if __name__ == "__main__":
    unittest.main()
