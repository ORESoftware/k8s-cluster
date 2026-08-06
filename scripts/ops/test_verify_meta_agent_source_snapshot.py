#!/usr/bin/env python3
from __future__ import annotations

import base64
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))

import verify_meta_agent_source_snapshot as verifier  # noqa: E402


class MetaAgentSourceSnapshotVerifierTests(unittest.TestCase):
    def test_immutable_release_constants_are_exact(self) -> None:
        self.assertEqual(
            verifier.SOURCE_SHA,
            "55ee15c190b7cfa4e075f6984c7cb551acd4b9d3",
        )
        self.assertEqual(
            verifier.BUNDLE_SHA256,
            "1ddaa03743b864348162149b7d2d2e2dce7eab585cf092ea14547c647fcec031",
        )
        self.assertEqual(
            verifier.PUBLISHER_SHA256,
            "e2fe6eaa622db02a54f83e27a822f64ad4b54971c883f97bbda4ac0a4db5d278",
        )
        self.assertEqual(
            verifier.EXPECTED_HEADS,
            {
                "HEAD": "789d48039da232faed985d4f8de176959f117e08",
                "refs/heads/main": "4d6ec3ad0ec7b688f0e777129eee7e0f0d999df1",
                "refs/heads/agent/den-1057-meta-agent-control-plane": "789d48039da232faed985d4f8de176959f117e08",
            },
        )
        self.assertEqual(
            verifier.EXPECTED_HEADS["HEAD"],
            verifier.EXPECTED_HEADS[verifier.EXPECTED_FEATURE_REF],
        )

    def test_require_sha_accepts_only_lowercase_exact_sha(self) -> None:
        sha = "a" * 40
        self.assertEqual(
            verifier.require_sha(sha, stage="test", label="sha"),
            sha,
        )
        for invalid in (None, 42, "a" * 39, "A" * 40, "g" * 40, "a" * 41):
            with self.subTest(invalid=invalid):
                with self.assertRaises(verifier.SnapshotError):
                    verifier.require_sha(invalid, stage="test", label="sha")

    def test_asset_selection_is_lexical_exact_and_blob_only(self) -> None:
        payload = {
            "truncated": False,
            "tree": [
                {
                    "path": "scripts/critical-org-fleet/assets/meta.part10",
                    "type": "blob",
                    "sha": "b" * 40,
                },
                {
                    "path": "scripts/critical-org-fleet/assets/other.part00",
                    "type": "blob",
                    "sha": "c" * 40,
                },
                {
                    "path": "scripts/critical-org-fleet/assets/meta.part02",
                    "type": "blob",
                    "sha": "a" * 40,
                },
            ],
        }
        self.assertEqual(
            verifier.select_asset_entries(payload),
            [
                ("scripts/critical-org-fleet/assets/meta.part02", "a" * 40),
                ("scripts/critical-org-fleet/assets/meta.part10", "b" * 40),
            ],
        )

        payload["tree"][0]["type"] = "tree"
        with self.assertRaisesRegex(verifier.SnapshotError, "not a blob"):
            verifier.select_asset_entries(payload)

    def test_asset_selection_rejects_truncation_empty_and_duplicates(self) -> None:
        with self.assertRaisesRegex(verifier.SnapshotError, "truncated"):
            verifier.select_asset_entries({"truncated": True, "tree": []})
        with self.assertRaisesRegex(verifier.SnapshotError, "no sealed"):
            verifier.select_asset_entries({"truncated": False, "tree": []})

        duplicate = {
            "truncated": False,
            "tree": [
                {
                    "path": "scripts/critical-org-fleet/assets/meta.part00",
                    "type": "blob",
                    "sha": "a" * 40,
                },
                {
                    "path": "scripts/critical-org-fleet/assets/meta.part00",
                    "type": "blob",
                    "sha": "b" * 40,
                },
            ],
        }
        with self.assertRaisesRegex(verifier.SnapshotError, "duplicate"):
            verifier.select_asset_entries(duplicate)

    def test_publisher_selection_requires_exactly_one_blob(self) -> None:
        entry = {
            "path": verifier.PUBLISHER_PATH,
            "type": "blob",
            "sha": "a" * 40,
        }
        self.assertEqual(
            verifier.select_publisher_entry({"tree": [entry]}),
            (verifier.PUBLISHER_PATH, "a" * 40),
        )
        for tree in ([], [entry, dict(entry)]):
            with self.subTest(tree=tree):
                with self.assertRaises(verifier.SnapshotError):
                    verifier.select_publisher_entry({"tree": tree})

    def test_github_blob_decode_verifies_transport_and_git_identity(self) -> None:
        content = b"sealed-source-segment\n"
        sha = verifier.git_blob_sha(content)
        payload = {
            "encoding": "base64",
            "content": base64.b64encode(content).decode("ascii"),
        }
        self.assertEqual(
            verifier.decode_github_blob(
                payload,
                expected_sha=sha,
                stage="decode",
                label="asset",
            ),
            content,
        )

        with self.assertRaisesRegex(verifier.SnapshotError, "identity mismatch"):
            verifier.decode_github_blob(
                payload,
                expected_sha="f" * 40,
                stage="decode",
                label="asset",
            )
        with self.assertRaisesRegex(verifier.SnapshotError, "transport base64"):
            verifier.decode_github_blob(
                {"encoding": "base64", "content": "***"},
                expected_sha=sha,
                stage="decode",
                label="asset",
            )

    def test_two_layer_bundle_decode_preserves_order_and_whitespace(self) -> None:
        bundle = b"binary\x00git\nbundle\xff"
        encoded = base64.b64encode(bundle)
        parts = [encoded[:5] + b"\n", encoded[5:12], b"\n" + encoded[12:]]
        self.assertEqual(verifier.decode_bundle_parts(parts), bundle)
        with self.assertRaisesRegex(verifier.SnapshotError, "not valid base64"):
            verifier.decode_bundle_parts([b"not***base64"])
        with self.assertRaisesRegex(verifier.SnapshotError, "empty"):
            verifier.decode_bundle_parts([])

    def test_bundle_head_parser_requires_exact_branches_and_symbolic_head(self) -> None:
        output = "\n".join(
            f"{sha} {ref}" for ref, sha in verifier.EXPECTED_HEADS.items()
        )
        self.assertEqual(verifier.parse_bundle_heads(output), verifier.EXPECTED_HEADS)

        without_head = "\n".join(
            f"{sha} {ref}"
            for ref, sha in verifier.EXPECTED_HEADS.items()
            if ref != "HEAD"
        )
        with self.assertRaisesRegex(verifier.SnapshotError, "bundle heads differ"):
            verifier.parse_bundle_heads(without_head)

        wrong_head = output.replace(
            f"{verifier.EXPECTED_FEATURE} HEAD",
            f"{'c' * 40} HEAD",
        )
        with self.assertRaisesRegex(verifier.SnapshotError, "bundle heads differ"):
            verifier.parse_bundle_heads(wrong_head)

        with self.assertRaisesRegex(verifier.SnapshotError, "bundle heads differ"):
            verifier.parse_bundle_heads(output + f"\n{'c' * 40} refs/heads/extra")
        with self.assertRaisesRegex(verifier.SnapshotError, "malformed"):
            verifier.parse_bundle_heads("bad-line")

    def test_workflow_token_preflight_rejects_absent_or_whitespace_values(self) -> None:
        for token in ("", "with space", "line\nbreak"):
            with self.subTest(token=token):
                with self.assertRaises(verifier.SnapshotError):
                    verifier.GitHubApi(token)
        self.assertIsInstance(verifier.GitHubApi("workflow-token"), verifier.GitHubApi)


if __name__ == "__main__":
    unittest.main()
