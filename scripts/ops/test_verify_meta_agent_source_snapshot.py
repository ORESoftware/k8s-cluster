#!/usr/bin/env python3
from __future__ import annotations

import base64
import hashlib
import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Any, Mapping

MODULE_PATH = Path(__file__).with_name("verify_meta_agent_source_snapshot.py")
SPEC = importlib.util.spec_from_file_location("meta_agent_source_snapshot", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"unable to load {MODULE_PATH}")
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


class FakeClient:
    def __init__(
        self,
        *,
        commit: Mapping[str, Any],
        trees: Mapping[str, Mapping[str, Any]],
        blobs: Mapping[str, Mapping[str, Any]],
    ) -> None:
        self._commit = commit
        self._trees = dict(trees)
        self._blobs = dict(blobs)

    def commit(self, sha: str) -> Mapping[str, Any]:
        if sha != self._commit["sha"]:
            raise AssertionError(f"unexpected commit request {sha}")
        return self._commit

    def tree(self, sha: str) -> Mapping[str, Any]:
        return self._trees[sha]

    def blob(self, sha: str) -> Mapping[str, Any]:
        return self._blobs[sha]


def run_git(cwd: Path, *arguments: str) -> str:
    return subprocess.run(
        ["git", *arguments],
        cwd=cwd,
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def blob_payload(value: bytes) -> Mapping[str, Any]:
    return {
        "encoding": "base64",
        "content": base64.b64encode(value).decode("ascii"),
    }


class SnapshotFixture:
    SOURCE_SHA = "a" * 40
    ROOT_TREE = "b" * 40
    SCRIPTS_TREE = "c" * 40
    FLEET_TREE = "d" * 40
    ASSETS_TREE = "e" * 40
    PUBLISHER_BLOB = "f" * 40
    PART_BLOBS = ("1" * 40, "2" * 40, "3" * 40)

    def __init__(self, root: Path) -> None:
        repository = root / "source"
        repository.mkdir()
        run_git(repository, "init", "--quiet", "-b", "main")
        run_git(repository, "config", "user.name", "Meta Agent Test")
        run_git(repository, "config", "user.email", "meta-agent-test@example.invalid")
        (repository / "README.md").write_text("baseline\n", encoding="utf-8")
        run_git(repository, "add", "README.md")
        run_git(repository, "commit", "--quiet", "-m", "baseline")
        self.main_sha = run_git(repository, "rev-parse", "HEAD")

        run_git(repository, "checkout", "--quiet", "-b", "agent/test-feature")
        (repository / "README.md").write_text("feature\n", encoding="utf-8")
        run_git(repository, "commit", "--quiet", "-am", "feature")
        self.feature_sha = run_git(repository, "rev-parse", "HEAD")
        run_git(repository, "checkout", "--quiet", "main")

        bundle_path = root / "fixture.bundle"
        run_git(
            repository,
            "bundle",
            "create",
            str(bundle_path),
            "refs/heads/main",
            "refs/heads/agent/test-feature",
        )
        self.bundle_bytes = bundle_path.read_bytes()
        encoded = base64.b64encode(self.bundle_bytes)
        split_one = len(encoded) // 3
        split_two = 2 * len(encoded) // 3
        self.parts = (
            encoded[:split_one],
            encoded[split_one:split_two],
            encoded[split_two:],
        )
        self.publisher_bytes = b"#!/usr/bin/env python3\nprint('fixture publisher')\n"

    def client(self, *, truncated_assets: bool = False) -> FakeClient:
        trees: dict[str, Mapping[str, Any]] = {
            self.ROOT_TREE: {
                "truncated": False,
                "tree": [
                    {"path": "scripts", "type": "tree", "sha": self.SCRIPTS_TREE},
                    {"path": "README.md", "type": "blob", "sha": "9" * 40},
                ],
            },
            self.SCRIPTS_TREE: {
                "truncated": False,
                "tree": [
                    {
                        "path": "critical-org-fleet",
                        "type": "tree",
                        "sha": self.FLEET_TREE,
                    }
                ],
            },
            self.FLEET_TREE: {
                "truncated": False,
                "tree": [
                    {"path": "assets", "type": "tree", "sha": self.ASSETS_TREE},
                    {
                        "path": MODULE.PUBLISHER_NAME,
                        "type": "blob",
                        "sha": self.PUBLISHER_BLOB,
                    },
                ],
            },
            self.ASSETS_TREE: {
                "truncated": truncated_assets,
                "tree": [
                    {
                        "path": f"meta.part{index:02d}",
                        "type": "blob",
                        "sha": blob_sha,
                    }
                    for index, blob_sha in enumerate(self.PART_BLOBS)
                ]
                + [{"path": "ignored.txt", "type": "blob", "sha": "8" * 40}],
            },
        }
        blobs = {
            **{
                blob_sha: blob_payload(part)
                for blob_sha, part in zip(self.PART_BLOBS, self.parts, strict=True)
            },
            self.PUBLISHER_BLOB: blob_payload(self.publisher_bytes),
        }
        return FakeClient(
            commit={"sha": self.SOURCE_SHA, "tree": {"sha": self.ROOT_TREE}},
            trees=trees,
            blobs=blobs,
        )


class MetaAgentSourceSnapshotTests(unittest.TestCase):
    def test_reconstructs_two_layer_bundle_and_verifies_exact_refs(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            fixture = SnapshotFixture(root)
            output = root / "output"
            expected_heads = {
                "refs/heads/main": fixture.main_sha,
                "refs/heads/agent/test-feature": fixture.feature_sha,
            }
            result = MODULE.reconstruct_and_verify(
                client=fixture.client(),
                source_sha=fixture.SOURCE_SHA,
                expected_bundle_sha256=hashlib.sha256(fixture.bundle_bytes).hexdigest(),
                expected_publisher_sha256=hashlib.sha256(
                    fixture.publisher_bytes
                ).hexdigest(),
                expected_heads=expected_heads,
                output_dir=output,
            )

            self.assertEqual(result.asset_count, 3)
            self.assertEqual(result.heads, expected_heads)
            self.assertEqual(result.bundle_path.read_bytes(), fixture.bundle_bytes)
            self.assertEqual(result.publisher_path.read_bytes(), fixture.publisher_bytes)
            self.assertEqual(result.bundle_path.stat().st_mode & 0o777, 0o600)
            self.assertEqual(result.publisher_path.stat().st_mode & 0o777, 0o600)
            evidence = json.loads(result.sanitized_json())
            self.assertEqual(evidence["status"], "verified")
            self.assertEqual(evidence["asset_count"], 3)
            self.assertNotIn("token", result.sanitized_json().lower())

    def test_rejects_truncated_bounded_tree(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            fixture = SnapshotFixture(root)
            with self.assertRaisesRegex(
                MODULE.VerificationError, "assets tree response is truncated"
            ):
                MODULE.reconstruct_and_verify(
                    client=fixture.client(truncated_assets=True),
                    source_sha=fixture.SOURCE_SHA,
                    expected_bundle_sha256=hashlib.sha256(
                        fixture.bundle_bytes
                    ).hexdigest(),
                    expected_publisher_sha256=hashlib.sha256(
                        fixture.publisher_bytes
                    ).hexdigest(),
                    expected_heads={
                        "refs/heads/main": fixture.main_sha,
                        "refs/heads/agent/test-feature": fixture.feature_sha,
                    },
                    output_dir=root / "output",
                )

    def test_rejects_exact_ref_mismatch(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            fixture = SnapshotFixture(root)
            with self.assertRaisesRegex(
                MODULE.VerificationError,
                "bundle refs do not exactly match the reviewed ref inventory",
            ):
                MODULE.reconstruct_and_verify(
                    client=fixture.client(),
                    source_sha=fixture.SOURCE_SHA,
                    expected_bundle_sha256=hashlib.sha256(
                        fixture.bundle_bytes
                    ).hexdigest(),
                    expected_publisher_sha256=hashlib.sha256(
                        fixture.publisher_bytes
                    ).hexdigest(),
                    expected_heads={"refs/heads/main": fixture.main_sha},
                    output_dir=root / "output",
                )


if __name__ == "__main__":
    unittest.main()
