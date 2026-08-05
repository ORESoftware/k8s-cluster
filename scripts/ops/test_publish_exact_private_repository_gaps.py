#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import os
from pathlib import Path
import sys
import unittest


ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "scripts/ops/publish_exact_private_repository_gaps.py"
os.environ.setdefault("GH_TOKEN", "unit-test-installation-token")
SPEC = importlib.util.spec_from_file_location("exact_private_repository_gaps", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"unable to load {MODULE_PATH}")
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


def record(full_name: str, commit: str = "a" * 40) -> dict[str, object]:
    owner, name = full_name.split("/", 1)
    return {
        "org": owner,
        "name": name,
        "full_name": full_name,
        "default_branch": "main",
        "commit": commit,
        "visibility": "private",
        "files": 1,
        "gitlinks": 0,
    }


class ExactPrivateRepositoryGapTests(unittest.TestCase):
    def test_hypesiege_selection_is_exact_ordered_and_cannot_expand(self) -> None:
        records = [
            record("hypesiege/hypesiege-scheduler.rs", "3" * 40),
            record("hypesiege/hypesiege-publishing-worker.rs", "2" * 40),
            record("hypesiege/hypesiege-analytics.rs", "1" * 40),
            record("hypesiege/unreviewed.rs", "4" * 40),
        ]
        selected = MODULE.selected_records(records, "hypesiege")
        self.assertEqual(
            [item["full_name"] for item in selected],
            list(MODULE.EXPECTED_REPOSITORIES["hypesiege"]),
        )
        self.assertNotIn("hypesiege/unreviewed.rs", {item["full_name"] for item in selected})

    def test_streempilot_selection_accepts_sealed_lowercase_owner(self) -> None:
        sealed = record("streempilot/streempilot-media-router.rs")
        selected = MODULE.selected_records([sealed], "StreemPilot")
        self.assertEqual(selected, [sealed])
        self.assertEqual(
            MODULE.EXPECTED_REPOSITORIES["StreemPilot"],
            ("StreemPilot/streempilot-media-router.rs",),
        )

    def test_streempilot_selection_fails_closed_on_missing_or_cross_org_identity(self) -> None:
        with self.assertRaisesRegex(RuntimeError, "missing from sealed manifest"):
            MODULE.selected_records([], "StreemPilot")

        cross_org = record("other/streempilot-media-router.rs")
        cross_org["full_name"] = "StreemPilot/streempilot-media-router.rs"
        # The exact full name remains the authority; a stale auxiliary org field
        # cannot expand or redirect the repository boundary.
        selected = MODULE.selected_records([cross_org], "StreemPilot")
        self.assertEqual(
            [item["full_name"] for item in selected],
            ["StreemPilot/streempilot-media-router.rs"],
        )

        escaped = record("other/streempilot-media-router.rs")
        original = MODULE.EXPECTED_REPOSITORIES["StreemPilot"]
        try:
            MODULE.EXPECTED_REPOSITORIES["StreemPilot"] = (
                "other/streempilot-media-router.rs",
            )
            with self.assertRaisesRegex(RuntimeError, "escaped organization boundary"):
                MODULE.selected_records([escaped], "StreemPilot")
        finally:
            MODULE.EXPECTED_REPOSITORIES["StreemPilot"] = original

    def test_case_insensitive_duplicate_or_invalid_identity_is_rejected(self) -> None:
        canonical = "StreemPilot/streempilot-media-router.rs"
        sealed = "streempilot/streempilot-media-router.rs"
        with self.assertRaisesRegex(RuntimeError, "duplicate repository identity"):
            MODULE.selected_records([record(canonical), record(sealed)], "StreemPilot")

        invalid_commit = record(canonical, "A" * 40)
        with self.assertRaisesRegex(RuntimeError, "invalid commit identity"):
            MODULE.selected_records([invalid_commit], "StreemPilot")

        invalid_branch = record(canonical)
        invalid_branch["default_branch"] = "master"
        with self.assertRaisesRegex(RuntimeError, "must use main"):
            MODULE.selected_records([invalid_branch], "StreemPilot")

    def test_source_has_no_public_creation_or_force_update_path(self) -> None:
        source = MODULE_PATH.read_text(encoding="utf-8")
        self.assertNotIn('"private": False', source)
        self.assertNotIn("--visibility " + "public", source)
        self.assertNotIn("git push --" + "force", source)
        self.assertNotIn("gh repo edit", source)
        self.assertIn("verify_preserved_existing", source)
        self.assertIn("refusing to publish repository outside exact allowlist", source)
        self.assertNotIn('"repository_count": len(selected)', source)
        self.assertIn("json.dumps(execution_manifest", source)
        self.assertIn('"--repository"', source)
        self.assertIn("casefold()", source)
        self.assertIn('"full_name": repository_full_name', source)


if __name__ == "__main__":
    unittest.main()
