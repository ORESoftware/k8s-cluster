#!/usr/bin/env python3
from __future__ import annotations

import argparse
from pathlib import Path

OLD = '''def ensure_tag(api: GitHub, spec: RepositorySpec, main_sha: str) -> str:
    tag = "v0.1.0"
    observed = tag_ref(api, spec, tag)
    if observed is None:
        status, _ = api.post(
            f"/repos/{spec.full_name}/git/refs",
            {"ref": f"refs/tags/{tag}", "sha": main_sha},
        )
        if status != 201:
            raise RuntimeError(f"create {tag} failed for {spec.full_name}: HTTP {status}")
        observed = tag_ref(api, spec, tag)
    if observed != main_sha:
        raise RuntimeError(
            f"immutable initial tag mismatch for {spec.full_name}: {observed} != {main_sha}"
        )
    return observed
'''

NEW = '''def ensure_tag(api: GitHub, spec: RepositorySpec, main_sha: str) -> str:
    tag = "v0.1.0"
    observed = tag_ref(api, spec, tag)
    if observed is None:
        status, _ = api.post(
            f"/repos/{spec.full_name}/git/refs",
            {"ref": f"refs/tags/{tag}", "sha": main_sha},
        )
        if status != 201:
            raise RuntimeError(f"create {tag} failed for {spec.full_name}: HTTP {status}")
        for attempt in range(12):
            observed = tag_ref(api, spec, tag)
            if observed is not None:
                break
            time.sleep(min(0.25 * (attempt + 1), 1.5))
    if observed != main_sha:
        raise RuntimeError(
            f"immutable initial tag mismatch for {spec.full_name}: {observed} != {main_sha}"
        )
    return observed
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
            "refusing unexpected Elenkos ensure_tag contract: "
            f"old={old_count} new={new_count} path={path}"
        )
    compile(path.read_text(encoding="utf-8"), str(path), "exec")
    return result


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Retry bounded reads after GitHub accepts an immutable tag ref."
    )
    parser.add_argument(
        "--publisher",
        type=Path,
        default=Path("scripts/ops/publish_elenkos_fleet_20260819.py"),
    )
    args = parser.parse_args()
    result = patch(args.publisher)
    print(
        "ELENKOS_TAG_VISIBILITY_PATCHED "
        f"path={args.publisher} result={result} attempts=12"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
