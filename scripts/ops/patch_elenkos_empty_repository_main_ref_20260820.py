#!/usr/bin/env python3
from __future__ import annotations

import argparse
from pathlib import Path

OLD = '''    status, document = api.get(f"/repos/{spec.full_name}/git/ref/heads/main", allow=(404,))
    if status == 404:
        return None
'''

NEW = '''    status, document = api.get(
        f"/repos/{spec.full_name}/git/ref/heads/main", allow=(404, 409)
    )
    if status == 404:
        return None
    if status == 409:
        if (
            not isinstance(document, dict)
            or document.get("message") != "Git Repository is empty."
        ):
            raise RuntimeError(
                f"unexpected main-ref conflict for {spec.full_name}: {document!r}"
            )
        return None
'''


def patch(path: Path) -> str:
    text = path.read_text(encoding="utf-8")
    old_count = text.count(OLD)
    new_count = text.count(NEW)

    if old_count == 0 and new_count == 1:
        result = "already-applied"
    elif old_count == 1 and new_count == 0:
        text = text.replace(OLD, NEW, 1)
        path.write_text(text, encoding="utf-8")
        result = "applied"
    else:
        raise RuntimeError(
            "refusing unexpected Elenkos publisher main_ref contract: "
            f"old={old_count} new={new_count} path={path}"
        )

    compile(path.read_text(encoding="utf-8"), str(path), "exec")
    return result


def main() -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Teach the Elenkos publisher that GitHub returns HTTP 409 for the "
            "main ref of a newly-created, empty repository."
        )
    )
    parser.add_argument(
        "--publisher",
        type=Path,
        default=Path("scripts/ops/publish_elenkos_fleet_20260819.py"),
    )
    args = parser.parse_args()
    result = patch(args.publisher)
    print(
        "ELENKOS_EMPTY_REPOSITORY_MAIN_REF_PATCHED "
        f"path={args.publisher} result={result} allow=404,409"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
