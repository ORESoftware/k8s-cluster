#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/ops-meta-agent-ephemeral-owner-broker.yml"
CONTRACT = ROOT / ".github/workflows/ops-meta-agent-ephemeral-owner-broker-contract.yml"
HELPER = ROOT / "scripts/ops/verify_meta_agent_source_snapshot.py"
HELPER_TEST = ROOT / "scripts/ops/test_verify_meta_agent_source_snapshot.py"
DOC = ROOT / "docs/operations/meta-agent-ephemeral-credential-publication.md"


class MetaAgentEphemeralOwnerBrokerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = WORKFLOW.read_text(encoding="utf-8")
        cls.contract = CONTRACT.read_text(encoding="utf-8")
        cls.helper = HELPER.read_text(encoding="utf-8")
        cls.helper_test = HELPER_TEST.read_text(encoding="utf-8")
        cls.doc = DOC.read_text(encoding="utf-8")

    def test_trigger_and_permissions_are_metadata_only_and_bounded(self) -> None:
        self.assertIn("pull_request_target:", self.workflow)
        self.assertIn("branches: [main]", self.workflow)
        self.assertIn("- .github/meta-agent-ephemeral-publish-trigger", self.workflow)
        permission_match = re.search(
            r"(?ms)^permissions:\n(?P<body>(?:  [^\n]+\n)+)", self.workflow
        )
        self.assertIsNotNone(permission_match)
        assert permission_match is not None
        self.assertEqual(
            {line.strip() for line in permission_match.group("body").splitlines()},
            {
                "contents: read",
                "issues: write",
                "pull-requests: write",
                "statuses: write",
            },
        )
        self.assertNotIn("id-token:", self.workflow)
        self.assertNotIn("actions/checkout@", self.workflow)

    def test_carrier_identity_and_shape_are_exact(self) -> None:
        required = (
            "github.event.pull_request.draft == true",
            "github.event.pull_request.user.login == 'ORESoftware'",
            "github.event.pull_request.head.repo.full_name == github.repository",
            "startsWith(github.event.pull_request.head.ref, 'agent/meta-agent-ephemeral-publish-')",
            "DO NOT MERGE: publish exact Meta Agent repository with encrypted credential",
            ".commits == 1",
            ".changed_files == 1",
            ".additions == 4",
            ".deletions == 0",
            '.[0].filename == $path',
            '.[0].status == "added"',
            '.[0].changes == 4',
        )
        for snippet in required:
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_carrier_is_ancestor_safe_and_pins_reviewed_artifacts(self) -> None:
        required = (
            'test "$marker_main" = "$parent_sha"',
            '(.status == "ahead" or .status == "identical") and .behind_by == 0',
            "SOURCE_SHA: 55ee15c190b7cfa4e075f6984c7cb551acd4b9d3",
            "SOURCE_HELPER_BLOB_SHA: 600e3d46c7604573af29a125ebfd43f0178844e3",
            "BUNDLE_SHA256: 1ddaa03743b864348162149b7d2d2e2dce7eab585cf092ea14547c647fcec031",
            "PUBLISHER_SHA256: e2fe6eaa622db02a54f83e27a822f64ad4b54971c883f97bbda4ac0a4db5d278",
            "EXPECTED_MAIN: 4d6ec3ad0ec7b688f0e777129eee7e0f0d999df1",
            "EXPECTED_FEATURE: 789d48039da232faed985d4f8de176959f117e08",
        )
        for snippet in required:
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_trusted_helper_is_blob_pinned_and_compiled(self) -> None:
        required = (
            "contents/scripts/ops/verify_meta_agent_source_snapshot.py?ref=${TRUSTED_SHA}",
            'test "$(jq -er \'.sha\' <<<"$helper_response")" = "$SOURCE_HELPER_BLOB_SHA"',
            'test "$(jq -er \'.encoding\' <<<"$helper_response")" = base64',
            'base64 --decode > "$helper"',
            'chmod 700 "$helper"',
            'python3 -m py_compile "$helper"',
        )
        for snippet in required:
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)
        self.assertIn('GH_TOKEN="$workflow_token" gh api', self.workflow)

    def test_source_preflight_completes_before_any_owner_challenge(self) -> None:
        helper_stage = self.workflow.index("stage=source-helper")
        preflight = self.workflow.index("stage=source-preflight")
        helper_call = self.workflow.index('GH_TOKEN="$workflow_token" python3 "$helper"')
        bundle_guard = self.workflow.index('test -s "$bundle"')
        challenge = self.workflow.index("stage=challenge-bootstrap")
        key_generation = self.workflow.index("openssl genpkey")
        owner_decryption = self.workflow.index("stage=decrypt-ciphertext")
        self.assertLess(helper_stage, preflight)
        self.assertLess(preflight, helper_call)
        self.assertLess(helper_call, bundle_guard)
        self.assertLess(bundle_guard, challenge)
        self.assertLess(challenge, key_generation)
        self.assertLess(key_generation, owner_decryption)
        self.assertIn("Immutable source preflight already passed.", self.workflow)
        self.assertNotIn("stage=reconstruct-reviewed-history", self.workflow)

    def test_source_preflight_arguments_are_exact(self) -> None:
        required = (
            "--repository ORESoftware/k8s-cluster",
            '--source-sha "$SOURCE_SHA"',
            '--bundle-sha256 "$BUNDLE_SHA256"',
            '--publisher-sha256 "$PUBLISHER_SHA256"',
            '--expected-head "refs/heads/main=${EXPECTED_MAIN}"',
            '--expected-head "refs/heads/${FEATURE_REF}=${EXPECTED_FEATURE}"',
            '--output-dir "$source_root"',
            'bundle="$source_root/meta-agent-control-plane-den-1057.bundle"',
            'publisher="$source_root/publish_meta_control_plane.py"',
        )
        for snippet in required:
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_challenge_uses_ephemeral_rsa_oaep_sha256(self) -> None:
        required = (
            "rsa_keygen_bits:3072",
            "openssl rand -hex 24",
            "meta-agent-credential-challenge:",
            "meta-agent-credential-response:",
            "rsa_padding_mode:oaep",
            "rsa_oaep_md:sha256",
            "rsa_mgf1_md:sha256",
        )
        for snippet in required:
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_response_is_owner_authored_newer_than_challenge_and_bounded(self) -> None:
        required = (
            'select(.id > $challenge_id)',
            'select(.user.login == "ORESoftware")',
            'select(.body | startswith($marker + "\\n"))',
            "test \"$(grep -c '^ciphertext-base64=' <<<\"$response_body\")\" -eq 1",
            'test "${#ciphertext}" -le 8192',
            "for _ in $(seq 1 180); do",
            "sleep 5",
        )
        for snippet in required:
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_plaintext_credential_is_memory_only_and_masked(self) -> None:
        self.assertIn('echo "::add-mask::$owner_token"', self.workflow)
        self.assertIn('export GH_TOKEN="$owner_token"', self.workflow)
        cleanup_match = re.search(
            r"(?ms)^          cleanup\(\) \{\n(?P<body>.*?)^          \}\n",
            self.workflow,
        )
        self.assertIsNotNone(cleanup_match)
        assert cleanup_match is not None
        cleanup = cleanup_match.group("body")
        for name in (
            "owner_token",
            "owner_login",
            "membership",
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GITHUB_REPOSITORY_ADMIN_TOKEN",
        ):
            with self.subTest(name=name):
                self.assertRegex(cleanup, rf"\b{name}\b")
        self.assertNotIn("GITHUB_ENV", self.workflow)
        self.assertNotIn("upload-artifact", self.workflow)
        self.assertNotRegex(self.workflow, r"ghp_[A-Za-z0-9]{20,}")
        self.assertNotRegex(self.workflow, r"github_pat_[A-Za-z0-9_]{20,}")

    def test_owner_identity_and_org_admin_are_verified_before_mutation(self) -> None:
        identity = self.workflow.index("stage=validate-owner-identity")
        membership = self.workflow.index("stage=validate-owner-membership")
        publication = self.workflow.index('python3 "$publisher" "$bundle"')
        self.assertLess(identity, publication)
        self.assertLess(membership, publication)
        self.assertIn('test "$owner_login" = ORESoftware', self.workflow)
        self.assertIn('test "$membership" = admin:active', self.workflow)

    def test_helper_walks_bounded_non_recursive_trees(self) -> None:
        required = (
            'path="scripts"',
            'path="critical-org-fleet"',
            'path="assets"',
            "path=PUBLISHER_NAME",
            "ASSET_NAME_PATTERN.fullmatch",
            "client.tree(root_tree_sha)",
            "client.tree(scripts_sha)",
            "client.tree(fleet_sha)",
            "client.tree(assets_sha)",
            'payload.get("truncated") is True',
        )
        for snippet in required:
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.helper)
        self.assertNotIn("recursive=1", self.helper)
        self.assertNotIn("git fetch", self.helper)
        self.assertNotIn("git clone", self.helper)

    def test_helper_decodes_both_layers_and_verifies_digest(self) -> None:
        outer_decode = (
            'encoded_bundle.extend(decode_blob(client.blob(blob_sha), f"assets/{path}"))'
        )
        inner_decode = (
            "bundle_bytes = base64.b64decode(bytes(encoded_bundle), validate=False)"
        )
        digest = "observed_bundle_sha256 = sha256_bytes(bundle_bytes)"
        self.assertIn(outer_decode, self.helper)
        self.assertIn(inner_decode, self.helper)
        self.assertIn(digest, self.helper)
        self.assertLess(self.helper.index(outer_decode), self.helper.index(inner_decode))
        self.assertLess(self.helper.index(inner_decode), self.helper.index(digest))
        self.assertIn(
            "if observed_bundle_sha256 != expected_bundle_sha256:", self.helper
        )

    def test_helper_verifies_bundle_in_repository_context_and_exact_refs(self) -> None:
        init = self.helper.index('run_git(["init", "--bare", "--quiet"')
        verify = self.helper.index(
            'run_git(["-C", str(repository_context), "bundle", "verify"'
        )
        heads = self.helper.index("observed_heads = parse_bundle_heads(bundle_path)")
        exact = self.helper.index("if observed_heads != dict(expected_heads):")
        self.assertLess(init, verify)
        self.assertLess(verify, heads)
        self.assertLess(heads, exact)

    def test_contract_runs_diagnostics_unit_and_live_read_only_preflight(self) -> None:
        required = (
            "scripts/ops/verify_meta_agent_source_snapshot.py",
            "scripts/ops/test_meta_agent_ephemeral_owner_diagnostics.py",
            "scripts/ops/test_verify_meta_agent_source_snapshot.py",
            "Prove the immutable source snapshot before owner authorization",
            "GH_TOKEN: ${{ github.token }}",
            "python3 scripts/ops/verify_meta_agent_source_snapshot.py",
            "--source-sha 55ee15c190b7cfa4e075f6984c7cb551acd4b9d3",
            "--bundle-sha256 1ddaa03743b864348162149b7d2d2e2dce7eab585cf092ea14547c647fcec031",
            "--publisher-sha256 e2fe6eaa622db02a54f83e27a822f64ad4b54971c883f97bbda4ac0a4db5d278",
        )
        for snippet in required:
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.contract)
        self.assertIn("SnapshotFixture", self.helper_test)
        self.assertIn(
            "test_reconstructs_two_layer_bundle_and_verifies_exact_refs",
            self.helper_test,
        )

    def test_exact_target_and_live_refs_are_verified(self) -> None:
        required = (
            'python3 "$publisher" "$bundle"',
            'test "$main_sha" = "$EXPECTED_MAIN"',
            'test "$feature_sha" = "$EXPECTED_FEATURE"',
            '.visibility == "public"',
            ".private == false",
            '.default_branch == "main"',
        )
        for snippet in required:
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_no_force_or_generated_replacement_history_path_exists(self) -> None:
        combined = self.workflow + self.helper
        self.assertNotIn("git push --force", combined)
        self.assertNotIn("git push -f", combined)
        self.assertNotIn('"--force"', combined)
        self.assertIn("publish_meta_control_plane.py", combined)

    def test_review_pr_and_carrier_cleanup_use_the_workflow_token(self) -> None:
        self.assertIn(
            'gh api --method POST "repos/${TARGET_REPOSITORY}/pulls"', self.workflow
        )
        self.assertIn("Review PR: ${pr_url}", self.workflow)
        self.assertIn(
            'GH_TOKEN="$workflow_token" gh api --method PATCH', self.workflow
        )
        self.assertIn(
            '"repos/${REPOSITORY}/pulls/${PR_NUMBER}" -f state=closed',
            self.workflow,
        )

    def test_documentation_names_preflight_diagnostics_and_rotation(self) -> None:
        for phrase in (
            "never committed",
            "ephemeral RSA",
            "rotate the credential",
            "exact recovered Git history",
            "commit/tree/blob API snapshot",
            "two base64 layers",
            "bounded non-recursive tree walk",
            "source preflight",
            "initialized source repository",
            "decrypt-ciphertext",
            "validate-owner-identity",
            "validate-owner-membership",
            "Never repost an old ciphertext",
            "Linear",
        ):
            with self.subTest(phrase=phrase):
                self.assertIn(phrase, self.doc)


if __name__ == "__main__":
    unittest.main()
