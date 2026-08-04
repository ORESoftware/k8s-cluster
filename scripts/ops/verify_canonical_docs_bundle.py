#!/usr/bin/env python3
"""Verify the immutable Canonical Docs repository bundle without credentials."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import re
import stat
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path, PurePosixPath
from typing import Sequence

BUNDLE_SHA256 = "3169c190a11f8889ca0a29d5db58acabae1e3b887cc302407ccc350d3a461828"
MAIN_SHA = "1848835599049ca41f68a079b5ac04f7d360fe87"
MAIN_TREE_SHA = "06fc9f6856015679b57489f03def643c8182537b"
FEATURE_REF = "agent/den-1049-repository-baseline"
FEATURE_SHA = "54aa2efcbcfd21020614cbecccea5a907ead813f"
FEATURE_TREE_SHA = "b938d01e1cf8b8270cdc2d7b1eb9691ac4254ba8"
BUSINESS_PLAN_SHA256 = "b3bfd4d8596adffd3ed93ef3f530c46c5710f2ed6e6b9bff2929943628c22fe7"
EXPECTED_ASSETS = tuple(f"canonical-docs.part{index:03d}" for index in range(4))
EXPECTED_HEADS = {
    "refs/heads/main": MAIN_SHA,
    f"refs/heads/{FEATURE_REF}": FEATURE_SHA,
}
EXPECTED_MAIN_FILES = (".gitignore", "README.md", "docs/business-plan.md")
BASE64_PATTERN = re.compile(r"^[A-Za-z0-9+/]*={0,2}$")


class VerificationError(RuntimeError):
    """The sealed source does not match its reviewed contract."""


def fail(message: str) -> None:
    raise VerificationError(message)


def run(args: Sequence[str], *, cwd: Path | None = None) -> str:
    completed = subprocess.run(
        list(args),
        cwd=cwd,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    if completed.returncode != 0:
        fail(
            f"command failed ({completed.returncode}): {' '.join(args)}\n"
            f"{completed.stdout[:4000]}"
        )
    return completed.stdout


def regular_file(path: Path, root: Path) -> Path:
    try:
        metadata = path.lstat()
    except OSError as error:
        fail(f"cannot inspect {path}: {error}")
    if not stat.S_ISREG(metadata.st_mode):
        fail(f"asset is not a regular file: {path.name}")
    try:
        resolved = path.resolve(strict=True)
        resolved.relative_to(root)
    except (OSError, ValueError) as error:
        fail(f"asset escapes source root: {path.name}: {error}")
    return resolved


def read_assets(asset_dir: Path) -> tuple[list[str], bytes]:
    try:
        root = asset_dir.resolve(strict=True)
    except OSError as error:
        fail(f"cannot resolve asset directory: {error}")
    if not root.is_dir() or root.is_symlink():
        fail("asset directory must be a real directory")

    observed = sorted(path.name for path in root.glob("canonical-docs.part*"))
    if tuple(observed) != EXPECTED_ASSETS:
        fail(f"asset inventory mismatch: {observed!r}")

    chunks: list[str] = []
    for name in observed:
        path = regular_file(root / name, root)
        try:
            text = path.read_text(encoding="ascii")
        except (OSError, UnicodeError) as error:
            fail(f"cannot read {name} as ASCII: {error}")
        if not text.endswith("\n") or text.count("\n") != 1:
            fail(f"{name} must contain one base64 line and one final newline")
        chunk = text[:-1]
        if not chunk or BASE64_PATTERN.fullmatch(chunk) is None:
            fail(f"{name} is not canonical base64")
        if "=" in chunk and name != observed[-1]:
            fail(f"padding appears before the final asset: {name}")
        chunks.append(chunk)

    encoded = "".join(chunks)
    try:
        bundle = base64.b64decode(encoded, validate=True)
    except ValueError as error:
        fail(f"sealed bundle base64 is invalid: {error}")
    digest = hashlib.sha256(bundle).hexdigest()
    if digest != BUNDLE_SHA256:
        fail(f"bundle digest mismatch: {digest} != {BUNDLE_SHA256}")
    return observed, bundle


def parse_heads(output: str) -> dict[str, str]:
    heads: dict[str, str] = {}
    for raw_line in output.splitlines():
        if not raw_line.strip():
            continue
        fields = raw_line.split()
        if len(fields) != 2 or re.fullmatch(r"[0-9a-f]{40}", fields[0]) is None:
            fail(f"malformed bundle head: {raw_line!r}")
        sha, name = fields
        if name in heads:
            fail(f"duplicate bundle head: {name}")
        heads[name] = sha
    return heads


def safe_extract(archive: Path, destination: Path) -> None:
    with tarfile.open(archive, mode="r:") as source:
        for member in source.getmembers():
            relative = PurePosixPath(member.name)
            if (
                relative.is_absolute()
                or not relative.parts
                or any(part in {"", ".", ".."} for part in relative.parts)
            ):
                fail(f"unsafe archive path: {member.name!r}")
            if not (member.isdir() or member.isreg()):
                fail(f"archive contains a non-regular entry: {member.name!r}")
        source.extractall(destination, filter="data")


def verify_bundle(bundle: bytes) -> dict[str, object]:
    with tempfile.TemporaryDirectory(prefix="canonical-docs-source-") as temporary:
        work = Path(temporary)
        bundle_path = work / "canonical-docs.bundle"
        bundle_path.write_bytes(bundle)

        verify_repository = work / "verify.git"
        run(["git", "init", "--bare", str(verify_repository)])
        run(["git", "-C", str(verify_repository), "bundle", "verify", str(bundle_path)])

        heads = parse_heads(run(["git", "bundle", "list-heads", str(bundle_path)]))
        if heads != EXPECTED_HEADS:
            fail(f"bundle head inventory mismatch: {heads!r}")

        bare = work / "source.git"
        run(["git", "clone", "--bare", str(bundle_path), str(bare)])

        main_tree = run(
            ["git", "--git-dir", str(bare), "rev-parse", f"{MAIN_SHA}^{{tree}}"]
        ).strip()
        feature_tree = run(
            ["git", "--git-dir", str(bare), "rev-parse", f"{FEATURE_SHA}^{{tree}}"]
        ).strip()
        feature_parent = run(
            ["git", "--git-dir", str(bare), "rev-parse", f"{FEATURE_SHA}^"]
        ).strip()
        if main_tree != MAIN_TREE_SHA:
            fail(f"main tree mismatch: {main_tree}")
        if feature_tree != FEATURE_TREE_SHA:
            fail(f"feature tree mismatch: {feature_tree}")
        if feature_parent != MAIN_SHA:
            fail(f"feature parent mismatch: {feature_parent}")

        main_files = tuple(
            line
            for line in run(
                ["git", "--git-dir", str(bare), "ls-tree", "-r", "--name-only", MAIN_SHA]
            ).splitlines()
            if line
        )
        if main_files != EXPECTED_MAIN_FILES:
            fail(f"original main inventory changed: {main_files!r}")

        business_plan = subprocess.run(
            [
                "git",
                "--git-dir",
                str(bare),
                "show",
                f"{MAIN_SHA}:docs/business-plan.md",
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        if business_plan.returncode != 0:
            fail(business_plan.stderr.decode(errors="replace")[:4000])
        business_digest = hashlib.sha256(business_plan.stdout).hexdigest()
        if business_digest != BUSINESS_PLAN_SHA256:
            fail(f"business-plan digest mismatch: {business_digest}")

        archive = work / "feature.tar"
        run(
            [
                "git",
                "--git-dir",
                str(bare),
                "archive",
                "--format=tar",
                f"--output={archive}",
                FEATURE_SHA,
            ]
        )
        feature_root = work / "feature"
        feature_root.mkdir()
        safe_extract(archive, feature_root)
        contract = run([sys.executable, "scripts/check_docs.py"], cwd=feature_root).strip()
        if not contract.startswith("documentation contract: PASS"):
            fail(f"unexpected documentation contract output: {contract}")

        return {
            "bundle_sha256": BUNDLE_SHA256,
            "heads": heads,
            "main_tree": main_tree,
            "feature_tree": feature_tree,
            "feature_parent": feature_parent,
            "main_files": list(main_files),
            "business_plan_sha256": business_digest,
            "documentation_contract": contract,
        }


def verify(asset_dir: Path) -> dict[str, object]:
    assets, bundle = read_assets(asset_dir)
    report = verify_bundle(bundle)
    report["assets"] = assets
    report["asset_count"] = len(assets)
    report["bundle_bytes"] = len(bundle)
    return report


def parse_args(argv: Sequence[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--asset-dir",
        type=Path,
        default=Path(__file__).resolve().parents[1] / "critical-org-fleet" / "assets",
    )
    parser.add_argument("--json-report", type=Path)
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    try:
        report = verify(args.asset_dir)
        if args.json_report is not None:
            args.json_report.write_text(
                json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
            )
    except (VerificationError, OSError, UnicodeError) as error:
        print(f"canonical-docs bundle verification failed: {error}", file=sys.stderr)
        return 1
    print(
        "canonical-docs bundle verification: PASS "
        f"assets={report['asset_count']} bytes={report['bundle_bytes']} "
        f"main={MAIN_SHA} feature={FEATURE_SHA}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
