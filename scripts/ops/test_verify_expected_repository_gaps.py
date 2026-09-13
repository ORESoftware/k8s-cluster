#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
from pathlib import Path
import sys
import unittest


ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "scripts/ops/verify_expected_repository_gaps.py"
SPEC = importlib.util.spec_from_file_location("verify_expected_repository_gaps", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"unable to load {MODULE_PATH}")
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


class ExpectedRepositoryGapVerifierTests(unittest.TestCase):
    def manifest(self) -> dict[str, object]:
        names = list(MODULE.EXPECTED_MISSING)
        names.extend(f"hypesiege/reviewed-{index}.rs" for index in range(28))
        records = [
            {
                "full_name": name,
                "commit": f"{index + 1:040x}",
                "visibility": "public",
            }
            for index, name in enumerate(names)
        ]
        return {
            "schema_version": 2,
            "repository_count": 32,
            "generator_sha256": MODULE.GENERATOR_SHA256,
            "organizations": {"hypesiege": 15, "streempilot": 17},
            "repositories": records,
        }

    def test_reviewed_manifest_accepts_exact_contract(self) -> None:
        records = MODULE.validate_manifest(self.manifest())
        self.assertEqual(len(records), 32)
        names = {record["full_name"] for record in records}
        self.assertTrue(set(MODULE.EXPECTED_MISSING).issubset(names))

    def test_reviewed_manifest_rejects_duplicate_identity(self) -> None:
        manifest = self.manifest()
        records = manifest["repositories"]
        assert isinstance(records, list)
        records[-1]["full_name"] = records[0]["full_name"]
        with self.assertRaisesRegex(MODULE.GapVerificationError, "duplicated"):
            MODULE.validate_manifest(manifest)

    def test_gap_set_must_be_exact(self) -> None:
        MODULE.assert_expected_missing(list(reversed(MODULE.EXPECTED_MISSING)))
        with self.assertRaisesRegex(MODULE.GapVerificationError, "gap set changed"):
            MODULE.assert_expected_missing(list(MODULE.EXPECTED_MISSING[:-1]))
        with self.assertRaisesRegex(MODULE.GapVerificationError, "gap set changed"):
            MODULE.assert_expected_missing(
                [*MODULE.EXPECTED_MISSING, "hypesiege/unreviewed-extra.rs"]
            )

    def test_private_main_state_requires_stable_id_and_exact_sha(self) -> None:
        repository = {
            "id": 123,
            "private": True,
            "visibility": "private",
            "default_branch": "main",
            "html_url": "https://github.com/hypesiege/example.rs",
        }
        reference = {"object": {"sha": "a" * 40}}
        state = MODULE.validate_repository_state(
            "hypesiege/example.rs", repository, reference
        )
        self.assertEqual(state["repository_id"], 123)
        self.assertEqual(state["main_sha"], "a" * 40)

        public = dict(repository, private=False, visibility="public")
        with self.assertRaisesRegex(MODULE.GapVerificationError, "not private"):
            MODULE.validate_repository_state(
                "hypesiege/example.rs", public, reference
            )
        wrong_branch = dict(repository, default_branch="dev")
        with self.assertRaisesRegex(MODULE.GapVerificationError, "not main"):
            MODULE.validate_repository_state(
                "hypesiege/example.rs", wrong_branch, reference
            )
        with self.assertRaisesRegex(MODULE.GapVerificationError, "exact main SHA"):
            MODULE.validate_repository_state(
                "hypesiege/example.rs", repository, {"object": {"sha": "bad"}}
            )

    def test_verifier_has_no_github_mutation_surface(self) -> None:
        source = MODULE_PATH.read_text(encoding="utf-8")
        self.assertIn('method="GET"', source)
        self.assertNotIn('method="POST"', source)
        self.assertNotIn('method="PATCH"', source)
        self.assertNotIn('method="DELETE"', source)
        self.assertNotIn("gh repo edit", source)


if __name__ == "__main__":
    unittest.main()
