#!/usr/bin/env python3
"""Create, normalize, bootstrap, and track the missing co-living repositories."""
from __future__ import annotations
import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Mapping
from coliving_bootstrap import bootstrap_files
from coliving_repository_publisher import TOKEN_SHAPE_RE, create_payload, patch_payload, publish
from coliving_repository_specs import EXPECTED_COUNTS, SPECS

def run(command: list[str], cwd: Path, env: Mapping[str, str] | None = None) -> None:
    completed = subprocess.run(command, cwd=cwd, env=None if env is None else dict(env), text=True, capture_output=True)
    if completed.returncode != 0:
        raise RuntimeError(
            f"command failed in {cwd}: {' '.join(command)}\nstdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )


def validate_allowlist() -> None:
    names = [spec.full_name for spec in SPECS]
    if len(names) != 33 or len(names) != len(set(names)):
        raise RuntimeError("repository allowlist must contain exactly 33 unique repositories")
    counts = {org: sum(spec.org == org for spec in SPECS) for org in EXPECTED_COUNTS}
    if counts != dict(EXPECTED_COUNTS):
        raise RuntimeError(f"repository allowlist count mismatch: {counts!r}")
    for spec in SPECS:
        payload = create_payload(spec)
        if payload["private"] is not True or payload["auto_init"] is not True:
            raise RuntimeError(f"unsafe create payload for {spec.full_name}")
        if patch_payload(spec)["visibility"] != "private":
            raise RuntimeError(f"unsafe visibility payload for {spec.full_name}")
        if not spec.description or len(spec.description) > 160:
            raise RuntimeError(f"invalid repository description length for {spec.full_name}")


def self_test() -> int:
    validate_allowlist()
    with tempfile.TemporaryDirectory(prefix="coliving-repository-publisher-") as temporary:
        root = Path(temporary)
        for spec in SPECS:
            repo = root / spec.org / spec.name
            repo.mkdir(parents=True)
            files = bootstrap_files(spec)
            for relative, content in files.items():
                path = repo / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(content, encoding="utf-8")
                if relative.startswith("scripts/"):
                    path.chmod(0o755)
            run([sys.executable, "scripts/verify_repository.py"], repo)
            if spec.kind in {"cli", "public-core", "mcp", "sidecar", "integrations", "worker", "web"}:
                if shutil.which("cargo"):
                    run(["cargo", "fmt", "--all", "--", "--check"], repo)
                    run(["cargo", "test", "--all-targets"], repo)
                else:
                    print(f"rust self-test skipped locally for {spec.full_name}: cargo unavailable")
            if spec.kind == "test":
                environment = os.environ.copy()
                environment["PYTHONPATH"] = str(repo / "src")
                run([sys.executable, "-m", "unittest", "discover", "-s", "tests", "-v"], repo, environment)
            for content in files.values():
                if TOKEN_SHAPE_RE.search(content):
                    raise RuntimeError(f"credential-shaped content generated for {spec.full_name}")
        print(f"co-living repository publisher self-test passed for {len(SPECS)} repositories")
    return 0


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument(
        "--evidence",
        type=Path,
        default=Path("ops/evidence/coliving-repositories/publication.json"),
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    return self_test() if args.self_test else publish(args.evidence)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:
        print(f"coliving-repository-publisher=failed reason={type(error).__name__}: {error}", file=sys.stderr)
        raise SystemExit(1)
