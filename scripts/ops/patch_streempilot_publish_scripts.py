#!/usr/bin/env python3
"""Harden generated StreemPilot publishers against stdin-consuming child commands."""
from __future__ import annotations

import argparse
from pathlib import Path


def replace_once(source: str, old: str, new: str, label: str) -> str:
    count = source.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    return source.replace(old, new, 1)


def patch_publish(path: Path) -> None:
    source = path.read_text(encoding="utf-8")
    source = replace_once(
        source,
        "while IFS=$'\\t' read -r full local_path description main_sha feature feature_sha title body_file; do",
        "while IFS=$'\\t' read -r full local_path description main_sha feature feature_sha title body_file <&3; do",
        "publish loop read",
    )
    source = replace_once(
        source,
        'done < <(python3 - "$MANIFEST" <<\'PY_MANIFEST\'',
        'done 3< <(python3 - "$MANIFEST" <<\'PY_MANIFEST\'',
        "publish loop descriptor",
    )
    path.write_text(source, encoding="utf-8")


def patch_verify(path: Path) -> None:
    source = path.read_text(encoding="utf-8")
    source = replace_once(
        source,
        "while IFS=$'\\t' read -r full main_sha feature feature_sha; do",
        "while IFS=$'\\t' read -r full main_sha feature feature_sha <&3; do",
        "verify loop read",
    )
    source = replace_once(
        source,
        "done < <(jq -r '.repositories[] | [.full_name,.main_sha,.feature_branch,.feature_sha] | @tsv' \"$MANIFEST\")",
        "done 3< <(jq -r '.repositories[] | [.full_name,.main_sha,.feature_branch,.feature_sha] | @tsv' \"$MANIFEST\")",
        "verify loop descriptor",
    )
    path.write_text(source, encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("fleet_root", type=Path)
    args = parser.parse_args()

    scripts = args.fleet_root.resolve() / "scripts"
    publish = scripts / "publish-all.sh"
    verify = scripts / "verify-remote.sh"
    if not publish.is_file() or not verify.is_file():
        raise SystemExit(f"generated publication scripts are missing under {scripts}")

    patch_publish(publish)
    patch_verify(verify)

    publish_text = publish.read_text(encoding="utf-8")
    verify_text = verify.read_text(encoding="utf-8")
    if "read -r full local_path description main_sha feature feature_sha title body_file <&3" not in publish_text:
        raise SystemExit("publish loop descriptor guard missing")
    if "done 3< <(python3" not in publish_text:
        raise SystemExit("publish loop descriptor source missing")
    if "read -r full main_sha feature feature_sha <&3" not in verify_text:
        raise SystemExit("verify loop descriptor guard missing")
    if "done 3< <(jq -r" not in verify_text:
        raise SystemExit("verify loop descriptor source missing")

    print("STREEMPILOT_PUBLISHER_STDIN_HARDENED")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
