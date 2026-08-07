#!/usr/bin/env python3
"""One-shot exact replacement patch for the five-gap encrypted publisher."""

from __future__ import annotations

from pathlib import Path


def replace_once(path: str, before: str, after: str, label: str) -> None:
    target = Path(path)
    source = target.read_text(encoding="utf-8")
    count = source.count(before)
    if count != 1:
        raise SystemExit(f"{label}: expected one anchor in {path}, found {count}")
    target.write_text(source.replace(before, after, 1), encoding="utf-8")


def main() -> int:
    publisher = "scripts/ops/publish_exact_private_repository_gaps.py"
    replace_once(
        publisher,
        "Publish only the four reviewed HypeSiege/StreemPilot repository gaps.",
        "Publish only the five reviewed HypeSiege/StreemPilot repository gaps.",
        "publisher scope",
    )
    replace_once(
        publisher,
        '    "StreemPilot": ("StreemPilot/streempilot-media-router.rs",),',
        '''    "StreemPilot": (
        "StreemPilot/streempilot-flutter-app",
        "StreemPilot/streempilot-media-router.rs",
    ),''',
        "StreemPilot exact allowlist",
    )

    runner = "scripts/ops/run_exact_private_repository_gaps_with_retry.py"
    replace_once(
        runner,
        '    "StreemPilot/streempilot-media-router.rs",\n',
        '    "StreemPilot/streempilot-flutter-app",\n    "StreemPilot/streempilot-media-router.rs",\n',
        "runner expected repositories",
    )
    replace_once(runner, "len(repositories) != 4", "len(repositories) != 5", "runner evidence length")
    replace_once(
        runner,
        '"expected_repository_count": 4,',
        '"expected_repository_count": 5,',
        "runner expected count",
    )
    replace_once(runner, 'total=4"', 'total=5"', "runner completion total")

    retry_test = "scripts/ops/test_run_exact_private_repository_gaps_with_retry.py"
    replace_once(
        retry_test,
        '        "StreemPilot/streempilot-media-router.rs": "1" * 40,\n',
        '        "StreemPilot/streempilot-flutter-app": "5" * 40,\n        "StreemPilot/streempilot-media-router.rs": "1" * 40,\n',
        "retry fixture digest",
    )
    replace_once(
        retry_test,
        '                ("StreemPilot/streempilot-media-router.rs",),\n',
        '''                (
                    "StreemPilot/streempilot-flutter-app",
                    "StreemPilot/streempilot-media-router.rs",
                ),
''',
        "retry StreemPilot evidence",
    )
    replace_once(
        retry_test,
        "def test_combined_evidence_requires_exact_four_private_sealed_heads(self) -> None:",
        "def test_combined_evidence_requires_exact_five_private_sealed_heads(self) -> None:",
        "retry test name",
    )
    replace_once(
        retry_test,
        'self.assertEqual(combined["expected_repository_count"], 4)',
        'self.assertEqual(combined["expected_repository_count"], 5)',
        "retry expected count assertion",
    )
    replace_once(
        retry_test,
        'self.assertEqual(combined["summary"]["created"], 4)',
        'self.assertEqual(combined["summary"]["created"], 5)',
        "retry created assertion",
    )

    publisher_test = "scripts/ops/test_publish_exact_private_repository_gaps.py"
    replace_once(
        publisher_test,
        '''        selected = MODULE.selected_records([cross_org], "StreemPilot")
        self.assertEqual(
            [item["full_name"] for item in selected],
            ["StreemPilot/streempilot-media-router.rs"],
        )
''',
        '''        flutter = record("StreemPilot/streempilot-flutter-app")
        selected = MODULE.selected_records([cross_org, flutter], "StreemPilot")
        self.assertEqual(
            [item["full_name"] for item in selected],
            list(MODULE.EXPECTED_REPOSITORIES["StreemPilot"]),
        )
''',
        "publisher selection fixture",
    )
    replace_once(
        publisher_test,
        'MODULE.selected_records([record(full_name), record(full_name)], "StreemPilot")',
        'MODULE.selected_records(\n                [record(full_name), record(full_name), record("StreemPilot/streempilot-flutter-app")],\n                "StreemPilot",\n            )',
        "publisher duplicate fixture",
    )
    replace_once(
        publisher_test,
        'MODULE.selected_records([invalid_commit], "StreemPilot")',
        'MODULE.selected_records(\n                [invalid_commit, record("StreemPilot/streempilot-flutter-app")],\n                "StreemPilot",\n            )',
        "publisher invalid commit fixture",
    )
    replace_once(
        publisher_test,
        'MODULE.selected_records([invalid_branch], "StreemPilot")',
        'MODULE.selected_records(\n                [invalid_branch, record("StreemPilot/streempilot-flutter-app")],\n                "StreemPilot",\n            )',
        "publisher invalid branch fixture",
    )

    workflow = ".github/workflows/ops-publish-exact-repository-gaps-encrypted-retry.yml"
    replace_once(
        workflow,
        "CONFIRMATION: publish-exact-four-private-gaps",
        "CONFIRMATION: publish-exact-five-private-gaps",
        "workflow confirmation",
    )
    replace_once(
        workflow,
        "Publish and verify the exact four private repositories",
        "Publish and verify the exact five private repositories",
        "workflow step title",
    )
    replace_once(
        workflow,
        "expected_repository_count == 4",
        "expected_repository_count == 5",
        "workflow expected count",
    )
    replace_once(
        workflow,
        ".summary.verified == 4",
        ".summary.verified == 5",
        "workflow verified count",
    )
    replace_once(
        workflow,
        '              "StreemPilot/streempilot-media-router.rs",\n',
        '              "StreemPilot/streempilot-flutter-app",\n              "StreemPilot/streempilot-media-router.rs",\n',
        "workflow expected names",
    )
    replace_once(
        workflow,
        "VERIFIED_ENCRYPTED_PAT_EXACT_GAPS 4/4",
        "VERIFIED_ENCRYPTED_PAT_EXACT_GAPS 5/5",
        "workflow completion marker",
    )
    replace_once(
        workflow,
        "exact four private HypeSiege/StreemPilot repositories",
        "exact five private HypeSiege/StreemPilot repositories",
        "workflow evidence body",
    )
    replace_once(
        workflow,
        "encrypted exact private repository publication'",
        "encrypted exact five-repository publication'",
        "workflow evidence title",
    )
    replace_once(
        workflow,
        "grep -qF 'StreemPilot/streempilot-media-router.rs' scripts/ops/run_exact_private_repository_gaps_with_retry.py\n",
        "grep -qF 'StreemPilot/streempilot-flutter-app' scripts/ops/run_exact_private_repository_gaps_with_retry.py\n          grep -qF 'StreemPilot/streempilot-media-router.rs' scripts/ops/run_exact_private_repository_gaps_with_retry.py\n",
        "workflow fifth allowlist assertion",
    )

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
