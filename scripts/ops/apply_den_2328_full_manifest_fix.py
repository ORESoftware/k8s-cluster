#!/usr/bin/env python3
"""One-shot fix: preserve the full sealed fleet manifest during exact publication."""

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
        '''        exact_manifest = {
            **execution_manifest,
            "repository_count": len(selected),
            "total_tracked_files": sum(int(record["files"]) for record in selected),
            "total_gitlinks": sum(int(record["gitlinks"]) for record in selected),
            "organizations": {organization.casefold(): len(selected)},
            "repositories": selected,
        }
        exact_manifest_path = work / f"{organization.casefold()}-exact-private-gaps.json"
        exact_manifest_path.write_text(
            json.dumps(exact_manifest, indent=2, sort_keys=True) + "\\n",
            encoding="utf-8",
        )
''',
        '''        # The sealed publisher validates the complete 32-repository ledger before
        # honoring its single --repository selector. Keep fleet totals and all
        # records intact here; the exact allowlist above controls only which
        # repository invocations and evidence rows are permitted.
        publisher_manifest_path = work / "private-fleet-execution.json"
        publisher_manifest_path.write_text(
            json.dumps(execution_manifest, indent=2, sort_keys=True) + "\\n",
            encoding="utf-8",
        )
''',
        "full publisher manifest",
    )
    replace_once(
        publisher,
        '                    str(exact_manifest_path),\n',
        '                    str(publisher_manifest_path),\n',
        "publisher manifest argument",
    )

    test_path = "scripts/ops/test_publish_exact_private_repository_gaps.py"
    replace_once(
        test_path,
        '''        self.assertIn("verify_preserved_existing", source)
        self.assertIn("refusing to publish repository outside exact allowlist", source)
''',
        '''        self.assertIn("verify_preserved_existing", source)
        self.assertIn("refusing to publish repository outside exact allowlist", source)
        self.assertNotIn('"repository_count": len(selected)', source)
        self.assertIn("json.dumps(execution_manifest", source)
        self.assertIn('"--repository"', source)
''',
        "full-manifest regression assertions",
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
