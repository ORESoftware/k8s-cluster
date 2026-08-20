#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("classify_repo_check_scope.py")
SPEC = importlib.util.spec_from_file_location("repo_check_scope", MODULE_PATH)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
REPO_CHECKS_WORKFLOW = REPOSITORY_ROOT / ".github/workflows/repo-checks.yml"


class RepoCheckScopeTests(unittest.TestCase):
    def test_non_pull_request_always_runs_private_contracts(self):
        result = MODULE.classify("push", ["docs/readme.md"])
        self.assertTrue(result["private_contracts_required"])
        self.assertFalse(result["credential_free_contract_only"])

    def test_governance_only_pull_request_skips_private_gitlinks(self):
        result = MODULE.classify(
            "pull_request",
            [
                "scripts/ops/build_org_project_docs_retry_registry.py",
                "ops/evidence/org-project-docs/audit.json",
            ],
        )
        self.assertTrue(result["governance_only"])
        self.assertFalse(result["credential_free_contract_only"])
        self.assertFalse(result["private_contracts_required"])

    def test_current_relationship_publisher_is_governance_only(self):
        result = MODULE.classify(
            "pull_request",
            [
                ".github/workflows/ops-current-org-dotgithub-relationships-ephemeral-publish.yml",
                "docs/operations/org-dotgithub-relationship-publication.md",
                "scripts/ci/classify_repo_check_scope.py",
                "scripts/ci/test_classify_repo_check_scope.py",
                "scripts/ops/org_repository_relationships_graph.py",
                "scripts/ops/org_repository_relationships_model.py",
                "scripts/ops/org_repository_relationships_render.py",
                "scripts/ops/org_repository_relationships_roles.py",
                "scripts/ops/publish_current_org_repository_relationships.py",
                "scripts/ops/publish_org_repository_relationships.py",
                "tests/ops/test_publish_current_org_repository_relationships.py",
            ],
        )
        self.assertTrue(result["governance_only"])
        self.assertFalse(result["credential_free_contract_only"])
        self.assertFalse(result["private_contracts_required"])
        self.assertEqual(
            "governance_only_no_private_gitlinks",
            result["reason"],
        )

    def test_den_3286_sealed_publisher_is_governance_only(self):
        result = MODULE.classify(
            "pull_request",
            [
                ".github/workflows/ops-publish-test-org-expansion-20260808.yml",
                "scripts/ci/classify_repo_check_scope.py",
                "scripts/ci/test_classify_repo_check_scope.py",
                "scripts/ops/publish_test_org_expansion_20260808.py.gz.b64",
                "scripts/ops/test-org-expansion-20260808.json.gz.b64",
            ],
        )
        self.assertTrue(result["governance_only"])
        self.assertFalse(result["credential_free_contract_only"])
        self.assertFalse(result["private_contracts_required"])
        self.assertEqual(
            "governance_only_no_private_gitlinks",
            result["reason"],
        )

    def test_den_3286_encrypted_recovery_is_governance_only(self):
        result = MODULE.classify(
            "pull_request",
            [
                ".github/workflows/ops-provision-den-3286-pat-recipient-20260809.yml",
                ".github/workflows/ops-publish-den-3286-encrypted-pat-20260809.yml",
                "ops/requests/den-3286-encrypted-pat-20260809.json",
                "ops/requests/den-3286-pat-recipient-20260809.json",
                "scripts/ci/classify_repo_check_scope.py",
                "scripts/ci/test_classify_repo_check_scope.py",
                "scripts/ops/provision_den_3286_pat_recipient_20260809.sh",
                "scripts/ops/publish_den_3286_with_encrypted_pat_20260809.sh",
            ],
        )
        self.assertTrue(result["governance_only"])
        self.assertFalse(result["credential_free_contract_only"])
        self.assertFalse(result["private_contracts_required"])
        self.assertEqual(
            "governance_only_no_private_gitlinks",
            result["reason"],
        )

    def test_den_2797_wave7_recovery_publisher_is_governance_only(self):
        result = MODULE.classify(
            "pull_request",
            [
                ".github/workflows/ops-den-2797-publish-wave7-recovery-once.yml",
                "scripts/ci/classify_repo_check_scope.py",
                "scripts/ci/test_classify_repo_check_scope.py",
                "scripts/den-2797-wave7-finalize.sh",
                "scripts/den-2797-wave7-prepare.sh",
                "scripts/den-2797-wave7-publish.sh",
                "scripts/den-2797-wave7-receive.sh",
                "scripts/den-2797-wave7-scrub.sh",
                "scripts/den-2797-wave7-validate.sh",
            ],
        )
        self.assertTrue(result["governance_only"])
        self.assertFalse(result["credential_free_contract_only"])
        self.assertFalse(result["private_contracts_required"])
        self.assertEqual(
            "governance_only_no_private_gitlinks",
            result["reason"],
        )

    def test_unreviewed_den_2797_wave7_publisher_requires_private_contracts(self):
        result = MODULE.classify(
            "pull_request",
            [
                ".github/workflows/ops-den-2797-publish-wave8-recovery-once.yml",
                "scripts/den-2797-wave8-publish.sh",
            ],
        )
        self.assertFalse(result["governance_only"])
        self.assertFalse(result["credential_free_contract_only"])
        self.assertTrue(result["private_contracts_required"])

    def test_unreviewed_den_3286_encrypted_recovery_requires_private_contracts(self):
        result = MODULE.classify(
            "pull_request",
            [
                ".github/workflows/ops-provision-den-3286-pat-recipient-20260810.yml",
                "ops/requests/den-3286-pat-recipient-20260810.json",
                "scripts/ops/provision_den_3286_pat_recipient_20260810.sh",
            ],
        )
        self.assertFalse(result["governance_only"])
        self.assertFalse(result["credential_free_contract_only"])
        self.assertTrue(result["private_contracts_required"])

    def test_unreviewed_publisher_path_still_requires_private_contracts(self):
        result = MODULE.classify(
            "pull_request",
            [
                ".github/workflows/ops-publish-test-org-expansion-20260809.yml",
                "scripts/ops/publish_test_org_expansion_20260809.py.gz.b64",
            ],
        )
        self.assertFalse(result["governance_only"])
        self.assertFalse(result["credential_free_contract_only"])
        self.assertTrue(result["private_contracts_required"])

    def test_den_319_rename_alias_contract_is_credential_free(self):
        result = MODULE.classify(
            "pull_request",
            [
                ".github/workflows/den-319-private-fleet-contracts.yml",
                ".github/workflows/repo-check-scope-contract.yml",
                "scripts/ci/classify_repo_check_scope.py",
                "scripts/ci/test_classify_repo_check_scope.py",
                "scripts/ops/repository_rename_alias_guard.py",
                "scripts/ops/test_repository_rename_alias_guard.py",
            ],
        )
        self.assertFalse(result["governance_only"])
        self.assertTrue(result["credential_free_contract_only"])
        self.assertFalse(result["private_contracts_required"])
        self.assertEqual(
            "credential_free_contract_only_no_private_gitlinks",
            result["reason"],
        )

    def test_scheduled_live_smoke_contract_is_credential_free(self):
        result = MODULE.classify(
            "pull_request",
            [
                ".github/workflows/athleto-ui-tests.yml",
                ".github/workflows/browser-mcp-external-smoke.yml",
                ".github/workflows/browser-mcp-public-e2e.yml",
                ".github/workflows/namespace-migration-contract.yml",
                "catalog/namespaces/migration-manifest.json",
                "remote/tests/general/browser-mcp-public-e2e.test.ts",
                "remote/tests/general/scheduled-live-smoke-contract.test.mjs",
                "remote/tests/ui/lib/harness.mjs",
                "remote/tests/ui/lib/live-targets.mjs",
                "scripts/ci/classify_repo_check_scope.py",
                "scripts/ci/test_classify_repo_check_scope.py",
                "tools/test_namespace_manifest.py",
            ],
        )
        self.assertFalse(result["governance_only"])
        self.assertTrue(result["credential_free_contract_only"])
        self.assertFalse(result["private_contracts_required"])
        self.assertEqual(
            "credential_free_contract_only_no_private_gitlinks",
            result["reason"],
        )

    def test_credential_free_contract_mixed_with_unknown_requires_private_contracts(self):
        result = MODULE.classify(
            "pull_request",
            [
                ".github/workflows/den-319-private-fleet-contracts.yml",
                "scripts/ops/test_repository_rename_alias_guard.py",
                "README.md",
            ],
        )
        self.assertFalse(result["governance_only"])
        self.assertFalse(result["credential_free_contract_only"])
        self.assertTrue(result["private_contracts_required"])

    def test_scope_control_change_requires_a_payload(self):
        result = MODULE.classify(
            "pull_request",
            [".github/workflows/repo-checks.yml"],
        )
        self.assertTrue(result["private_contracts_required"])
        self.assertFalse(result["credential_free_contract_only"])

    def test_scope_control_plus_governance_payload_is_credential_free(self):
        result = MODULE.classify(
            "pull_request",
            [
                ".github/workflows/repo-checks.yml",
                "scripts/ci/classify_repo_check_scope.py",
                "scripts/ci/test_classify_repo_check_scope.py",
                "scripts/ops/test_build_org_project_docs_retry_registry.py",
            ],
        )
        self.assertFalse(result["private_contracts_required"])
        self.assertFalse(result["credential_free_contract_only"])

    def test_remote_or_unknown_change_runs_private_contracts(self):
        for path in [
            "remote/tests/general/example.test.ts",
            "scripts/ci/init-submodules-with-report.sh",
            "README.md",
        ]:
            with self.subTest(path=path):
                result = MODULE.classify("pull_request", [path])
                self.assertTrue(result["private_contracts_required"])
                self.assertFalse(result["credential_free_contract_only"])

    def test_empty_pull_request_fails_closed(self):
        with self.assertRaises(MODULE.ScopeError):
            MODULE.classify("pull_request", [])

    def test_unsafe_paths_fail_closed(self):
        for path in ["../escape", "/absolute", "windows\\path"]:
            with self.subTest(path=path):
                with self.assertRaises(MODULE.ScopeError):
                    MODULE.classify("pull_request", [path])

    def test_repo_checks_use_three_dot_merge_base_scope(self):
        source = REPO_CHECKS_WORKFLOW.read_text(encoding="utf-8")
        self.assertIn(
            'git diff --name-only -z "${BASE_SHA}...${HEAD_SHA}"',
            source,
        )
        self.assertNotIn(
            'git diff --name-only -z "${BASE_SHA}" "${HEAD_SHA}"',
            source,
        )

    def test_three_dot_scope_excludes_commits_added_only_to_moving_base(self):
        with tempfile.TemporaryDirectory() as temporary:
            repository = Path(temporary) / "repository"

            def run(*command: str) -> subprocess.CompletedProcess[str]:
                return subprocess.run(
                    list(command),
                    cwd=repository if repository.exists() else None,
                    check=True,
                    capture_output=True,
                    text=True,
                )

            subprocess.run(
                ["git", "init", str(repository)],
                check=True,
                capture_output=True,
                text=True,
            )
            run("git", "config", "user.name", "scope fixture")
            run("git", "config", "user.email", "scope@example.invalid")
            (repository / "base.txt").write_text("base\n", encoding="utf-8")
            run("git", "add", "base.txt")
            run("git", "commit", "-m", "base")
            run("git", "branch", "-M", "main")

            run("git", "switch", "-c", "feature")
            (repository / "feature.txt").write_text("feature\n", encoding="utf-8")
            run("git", "add", "feature.txt")
            run("git", "commit", "-m", "feature")
            feature_sha = run("git", "rev-parse", "HEAD").stdout.strip()

            run("git", "switch", "main")
            (repository / "base-only.txt").write_text(
                "moving base\n",
                encoding="utf-8",
            )
            run("git", "add", "base-only.txt")
            run("git", "commit", "-m", "advance main")
            base_sha = run("git", "rev-parse", "HEAD").stdout.strip()

            changed = run(
                "git",
                "diff",
                "--name-only",
                "-z",
                f"{base_sha}...{feature_sha}",
            ).stdout.split("\0")
            self.assertEqual(
                [path for path in changed if path],
                ["feature.txt"],
            )


if __name__ == "__main__":
    unittest.main()
