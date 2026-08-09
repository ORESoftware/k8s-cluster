#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/ops-meta-agent-ephemeral-owner-broker.yml"
DOC = ROOT / "docs/operations/meta-agent-ephemeral-credential-publication.md"


class MetaAgentEphemeralOwnerDiagnosticsTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = WORKFLOW.read_text(encoding="utf-8")
        cls.doc = DOC.read_text(encoding="utf-8")

    def test_decrypt_and_authorization_stages_are_distinct_and_ordered(self) -> None:
        names = [
            "decrypt-ciphertext",
            "validate-owner-token-shape",
            "validate-owner-identity",
            "validate-owner-membership",
            "reconstruct-reviewed-history",
            "create-and-push-exact-repository",
        ]
        offsets = [self.workflow.index(f"stage={name}") for name in names]
        self.assertEqual(offsets, sorted(offsets))
        self.assertNotIn("stage=decrypt-and-validate-owner", self.workflow)

    def test_oaep_and_mgf1_are_both_explicit_sha256(self) -> None:
        decrypt = self.workflow[
            self.workflow.index("stage=decrypt-ciphertext") :
            self.workflow.index("stage=validate-owner-token-shape")
        ]
        self.assertIn("rsa_padding_mode:oaep", decrypt)
        self.assertIn("rsa_oaep_md:sha256", decrypt)
        self.assertIn("rsa_mgf1_md:sha256", decrypt)
        self.assertEqual(decrypt.count("openssl pkeyutl -decrypt"), 1)

    def test_decrypt_failure_is_conditional_and_does_not_disable_errexit(self) -> None:
        decrypt = self.workflow[
            self.workflow.index("stage=decrypt-ciphertext") :
            self.workflow.index("stage=validate-owner-token-shape")
        ]
        self.assertIn('if ! owner_token="$(\n', decrypt)
        self.assertIn("2>/dev/null", decrypt)
        self.assertIn("; then\n            false\n          fi", decrypt)
        self.assertNotIn("set +e", decrypt)

    def test_token_shape_is_checked_before_export_or_network_use(self) -> None:
        stage = self.workflow.index("stage=validate-owner-token-shape")
        shape = self.workflow.index('[[ "$owner_token" == ghp_* || "$owner_token" == github_pat_* ]]')
        mask = self.workflow.index('echo "::add-mask::$owner_token"')
        export = self.workflow.index('export GH_TOKEN="$owner_token"')
        identity = self.workflow.index("stage=validate-owner-identity")
        self.assertLess(stage, shape)
        self.assertLess(shape, mask)
        self.assertLess(mask, export)
        self.assertLess(export, identity)

    def test_identity_and_membership_requests_fail_closed_without_token_output(self) -> None:
        identity_block = self.workflow[
            self.workflow.index("stage=validate-owner-identity") :
            self.workflow.index("stage=reconstruct-reviewed-history")
        ]
        self.assertIn("if ! owner_login=", identity_block)
        self.assertIn('test "$owner_login" = ORESoftware', identity_block)
        self.assertIn("if ! membership=", identity_block)
        self.assertIn('test "$membership" = admin:active', identity_block)
        self.assertGreaterEqual(identity_block.count("2>/dev/null"), 2)
        self.assertNotIn("echo $owner_token", identity_block)
        self.assertNotIn("printf $owner_token", identity_block)

    def test_current_git_object_snapshot_survives_the_diagnostic_layer(self) -> None:
        reconstruction = self.workflow[
            self.workflow.index("stage=reconstruct-reviewed-history") :
            self.workflow.index("stage=create-and-push-exact-repository")
        ]
        for snippet in (
            'git/commits/${SOURCE_SHA}',
            'git/trees/${source_tree_sha}?recursive=1',
            'git/blobs/${asset_sha}',
            'bundle_base64="$work/meta-agent-control-plane-den-1057.bundle.b64"',
            'git -C "$source_root" bundle verify "$bundle"',
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, reconstruction)
        self.assertNotIn('fetch --depth=1 --no-tags origin "$SOURCE_SHA"', reconstruction)

    def test_cleanup_erases_derived_identity_state_and_plaintext_transport_remains_absent(self) -> None:
        self.assertIn(
            "unset owner_token owner_login membership GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN",
            self.workflow,
        )
        self.assertNotIn("GITHUB_ENV", self.workflow)
        self.assertNotIn("upload-artifact", self.workflow)
        self.assertNotIn("actions/cache", self.workflow)

    def test_operator_documentation_merges_snapshot_and_failure_boundaries(self) -> None:
        for phrase in (
            "RSA-OAEP-SHA256",
            "MGF1-SHA256",
            "commit/tree/blob API snapshot",
            "two base64 layers",
            "decrypt-ciphertext",
            "validate-owner-identity",
            "validate-owner-membership",
            "Never repost an old ciphertext",
        ):
            with self.subTest(phrase=phrase):
                self.assertIn(phrase, self.doc)


if __name__ == "__main__":
    unittest.main()
