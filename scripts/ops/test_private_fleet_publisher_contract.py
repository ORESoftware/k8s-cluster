#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import unittest

from repository_rename_alias_guard import (
    RepositoryRenameAliasError,
    RepositoryRenameAliasGuard,
)


ROOT = Path(__file__).resolve().parents[2]
WORKFLOW_PATH = (
    ROOT / ".github/workflows/ops-publish-missing-org-repositories-gh-profile.yml"
)
OBSERVER_PATH = ROOT / ".github/workflows/observe-private-fleet-publisher.yml"
PUBLISHER_PATH = ROOT / "scripts/ops/publish_missing_org_repositories_current.py"
REMOTE_STATE_PATH = ROOT / "scripts/ops/repository_fleet_remote_state.py"
CREATION_HELPER_PATH = ROOT / "scripts/ops/private_repository_creation.py"
ALIAS_GUARD_PATH = ROOT / "scripts/ops/repository_rename_alias_guard.py"


class PrivateFleetPublisherContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
        cls.observer = OBSERVER_PATH.read_text(encoding="utf-8")
        cls.publisher = PUBLISHER_PATH.read_text(encoding="utf-8")
        cls.remote_state = REMOTE_STATE_PATH.read_text(encoding="utf-8")
        cls.creation_helper = CREATION_HELPER_PATH.read_text(encoding="utf-8")
        cls.alias_guard = ALIAS_GUARD_PATH.read_text(encoding="utf-8")

    def test_workflow_retriggers_when_publisher_contracts_change(self) -> None:
        self.assertIn(
            "- scripts/ops/test_private_fleet_publisher_contract.py",
            self.workflow,
        )
        self.assertIn("push:\n    branches:\n      - main", self.workflow)

    def test_retrigger_is_serial_and_preserves_gap_aware_verification(self) -> None:
        self.assertIn(
            "group: ops-publish-missing-organization-repositories",
            self.workflow,
        )
        self.assertIn("cancel-in-progress: false", self.workflow)
        publication = self.workflow.index("stage=bounded-repository-publication")
        verification = self.workflow.index("stage=gap-aware-publication-verification")
        completion = self.workflow.index("stage=complete")
        self.assertLess(publication, verification)
        self.assertLess(verification, completion)
        self.assertIn(
            "created-exact-preserved-unchanged",
            self.workflow,
        )

    def test_publisher_observer_uses_exact_workflow_and_bounded_evidence(self) -> None:
        self.assertIn(
            "- Publish missing organization repositories from protected gh profile",
            self.observer,
        )
        self.assertIn("types:\n      - requested\n      - completed", self.observer)
        self.assertIn("actions: read", self.observer)
        self.assertIn("issues: write", self.observer)
        self.assertIn("failed_jobs", self.observer)
        self.assertNotIn("--log", self.observer)
        self.assertNotIn("AWS_SSM_INSTANCE_ID", self.observer)
        self.assertNotIn("GITHUB_REPOSITORY_ADMIN_TOKEN", self.observer)

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

    def test_publisher_uses_race_safe_private_creation_helper(self) -> None:
        self.assertIn(
            "ensure_private_repository as ensure_private_repository_with_api",
            self.publisher,
        )
        self.assertIn("ensure_private_repository_with_api(", self.publisher)
        self.assertIn("MODULE.api,", self.publisher)
        self.assertIn("alias_guard.api,", self.publisher)
        self.assertIn("_CREATE_CONFLICT_STATUSES", self.creation_helper)
        self.assertIn("reconciliation GET returned HTTP", self.creation_helper)
        self.assertNotIn("def _create_payload", self.publisher)

    def test_renamed_aliases_are_proved_recreated_and_preserved(self) -> None:
        self.assertIn("RepositoryRenameAliasGuard", self.publisher)
        self.assertIn("canonical_full_names=canonical_full_names", self.publisher)
        self.assertIn(
            "repository_lookup=alias_guard.repository_lookup", self.publisher
        )
        self.assertIn("alias_guard.verify_preserved()", self.publisher)
        self.assertIn("PRESERVE_RENAMED_TARGET", self.alias_guard)
        self.assertIn("VERIFIED_PRESERVED_RENAMED_TARGET", self.alias_guard)
        self.assertIn("_NoRedirectHandler", self.alias_guard)
        self.assertIn("redirect repository id mismatch", self.alias_guard)
        self.assertIn("is also a canonical fleet identity", self.alias_guard)
        self.assertNotIn('api("PATCH"', self.alias_guard)
        self.assertNotIn("--force", self.alias_guard)

    def test_missing_repositories_are_created_private_without_visibility_patch(self) -> None:
        self.assertIn('"private": True', self.creation_helper)
        self.assertIn('payload.get("private") is not True', self.creation_helper)
        self.assertIn(
            'payload.get("visibility") != "private"', self.creation_helper
        )
        self.assertNotIn('api("PATCH"', self.creation_helper)
        self.assertNotIn('MODULE.api("PATCH"', self.publisher)

    def test_publication_has_no_force_path(self) -> None:
        self.assertNotIn('"--force"', self.publisher)
        self.assertNotIn("--force", self.creation_helper)
        self.assertNotIn("git push --force", self.workflow)
        self.assertNotIn("git push -f", self.workflow)

    def test_created_sha_and_preserved_head_are_distinct_final_contracts(self) -> None:
        created = self.publisher.index("verify_created_repositories(")
        preserved = self.publisher.index("verify_preserved_existing(")
        renamed = self.publisher.index("alias_guard.verify_preserved()")
        created_evidence = self.publisher.index("VERIFIED_CREATED_PRIVATE")
        preserved_evidence = self.publisher.index("VERIFIED_PRESERVED_PRIVATE")
        self.assertLess(created, created_evidence)
        self.assertLess(preserved, preserved_evidence)
        self.assertLess(renamed, created_evidence)
        self.assertIn("created repository", self.remote_state)
        self.assertIn("main drift", self.remote_state)

    def test_missing_monorepo_is_ordered_after_leaf_repositories(self) -> None:
        self.assertIn('record.get("kind") == "monorepo"', self.remote_state)
        self.assertIn("Missing leaf histories must be published", self.remote_state)

    def test_obsolete_public_exact_root_finalizer_is_not_used(self) -> None:
        self.assertNotIn("finalize_missing_org_repositories.py", self.workflow)
        self.assertIn("created-exact-preserved-unchanged", self.workflow)


class RepositoryRenameAliasGuardTests(unittest.TestCase):
    requested = "example/service.rs"
    target = "example/portal.rs"
    target_head = "d" * 40

    @staticmethod
    def repository_payload(
        full_name: str,
        *,
        repository_id: int = 91,
    ) -> dict[str, object]:
        return {
            "id": repository_id,
            "full_name": full_name,
            "private": True,
            "visibility": "private",
            "default_branch": "main",
            "archived": False,
            "disabled": False,
        }

    def make_guard(
        self,
        *,
        target_name: str | None = None,
        target_id: int = 91,
        redirect_status: int = 301,
        redirect_location_id: int | None = None,
        canonical: set[str] | None = None,
    ) -> tuple[
        RepositoryRenameAliasGuard,
        dict[str, str],
        list[str],
    ]:
        actual_target = target_name or self.target
        payload = self.repository_payload(actual_target, repository_id=target_id)
        heads = {actual_target.casefold(): self.target_head}
        emitted: list[str] = []

        def api(
            method: str,
            path: str,
            body: dict[str, object] | None = None,
        ) -> tuple[int, object | None]:
            self.assertEqual(method, "GET")
            self.assertIsNone(body)
            if path == f"/repos/{self.requested}":
                return 200, payload
            if path == f"/repos/{actual_target}":
                return 200, payload
            raise AssertionError(f"unexpected API path: {path}")

        def main_ref(full_name: str) -> str | None:
            return heads.get(full_name.casefold())

        location_id = target_id if redirect_location_id is None else redirect_location_id
        guard = RepositoryRenameAliasGuard(
            api_base="https://api.github.com",
            token="test-token-not-a-real-credential",
            api=api,
            main_ref_lookup=main_ref,
            canonical_full_names=canonical or {self.requested},
            redirect_probe=lambda _: (
                redirect_status,
                f"https://api.github.com/repositories/{location_id}",
            ),
            emit=emitted.append,
        )
        return guard, heads, emitted

    def test_verified_same_owner_redirect_behaves_as_missing_and_is_preserved(self) -> None:
        guard, heads, emitted = self.make_guard()

        self.assertEqual(guard.repository_lookup(self.requested), (404, None))
        self.assertEqual(
            guard.api("GET", f"/repos/{self.requested}", None),
            (404, None),
        )
        self.assertEqual(len(guard.snapshots), 1)
        self.assertEqual(guard.snapshots[0].target_full_name, self.target)
        self.assertEqual(guard.snapshots[0].target_repository_id, 91)
        self.assertEqual(
            emitted,
            [
                "PRESERVE_RENAMED_TARGET example/service.rs -> "
                f"example/portal.rs id=91 head={self.target_head}"
            ],
        )

        guard.verify_preserved()
        self.assertEqual(
            emitted[-1],
            f"VERIFIED_PRESERVED_RENAMED_TARGET {self.target} {self.target_head}",
        )

        heads[self.target.casefold()] = "e" * 40
        with self.assertRaisesRegex(
            RepositoryRenameAliasError, "changed during alias recreation"
        ):
            guard.verify_preserved()

    def test_cross_owner_or_canonical_target_fails_closed(self) -> None:
        guard, _, _ = self.make_guard(target_name="other-owner/portal.rs")
        with self.assertRaisesRegex(RepositoryRenameAliasError, "owner mismatch"):
            guard.repository_lookup(self.requested)

        guard, _, _ = self.make_guard(
            canonical={self.requested, self.target}
        )
        with self.assertRaisesRegex(
            RepositoryRenameAliasError, "canonical fleet identity"
        ):
            guard.repository_lookup(self.requested)

    def test_unproved_or_id_mismatched_redirect_fails_closed(self) -> None:
        guard, _, _ = self.make_guard(redirect_status=200)
        with self.assertRaisesRegex(RepositoryRenameAliasError, "not a GitHub redirect"):
            guard.repository_lookup(self.requested)

        guard, _, _ = self.make_guard(redirect_location_id=92)
        with self.assertRaisesRegex(RepositoryRenameAliasError, "id mismatch"):
            guard.repository_lookup(self.requested)


if __name__ == "__main__":
    unittest.main()
