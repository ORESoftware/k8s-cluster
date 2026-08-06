#!/usr/bin/env python3
from __future__ import annotations

import hashlib
import re
import sys
from pathlib import Path

BASE_SHA256 = "3d1679596dfd85d045afd54790db88b63f061d3450156ac81a112c8d031c2120"

IDENTITY_GUARD = (
    "    _, identity = api.request('GET', '/user', expected=(200,))\n"
    "    if not isinstance(identity, dict) or not identity.get('login'):\n"
    "        raise RuntimeError('repository-administration token has no authenticated identity')\n"
)

REPLACEMENTS = (
    (
        "    parser.add_argument('--keep-workdir', type=pathlib.Path, help='materialize generated repositories here')\n",
        "    parser.add_argument('--keep-workdir', type=pathlib.Path, help='materialize generated repositories here')\n"
        "    parser.add_argument('--skip-build-validation', action='store_true', help='skip cargo validation after an immutable workflow preflight')\n",
    ),
    (
        "        if args.execute:\n"
        "            validate_generated_repositories(root)\n"
        "            token = os.environ.get('GITHUB_REPOSITORY_ADMIN_TOKEN', '').strip()\n",
        "        if args.execute:\n"
        "            if args.skip_build_validation:\n"
        "                expected_publisher = os.environ.get('NCC_PUBLISHER_SHA256', '').strip()\n"
        "                observed_publisher = hashlib.sha256(pathlib.Path(__file__).read_bytes()).hexdigest()\n"
        "                if not expected_publisher or expected_publisher != observed_publisher:\n"
        "                    raise RuntimeError('skip-build-validation requires the exact reviewed publisher digest')\n"
        "            else:\n"
        "                validate_generated_repositories(root)\n"
        "            token = os.environ.get('GITHUB_REPOSITORY_ADMIN_TOKEN', '').strip()\n",
    ),
)


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> int:
    if len(sys.argv) != 2:
        raise SystemExit("usage: patch_networking_components_publisher.py PUBLISHER")

    path = Path(sys.argv[1])
    observed = digest(path)
    if observed != BASE_SHA256:
        raise RuntimeError(f"base publisher digest mismatch: {observed}")

    source = path.read_text(encoding="utf-8")
    if source.count(IDENTITY_GUARD) != 1:
        raise RuntimeError("PAT identity guard is absent or ambiguous")
    if "/installation/repositories" in source:
        raise RuntimeError("installation-token compatibility must not be present in the PAT publisher")

    for old, new in REPLACEMENTS:
        if source.count(old) != 1:
            raise RuntimeError("publisher patch anchor is absent or ambiguous")
        source = source.replace(old, new)

    if source.count(IDENTITY_GUARD) != 1:
        raise RuntimeError("PAT identity guard changed unexpectedly")
    if source.count("--skip-build-validation") != 1:
        raise RuntimeError("skip-build-validation argument is absent or duplicated")
    if source.count("skip-build-validation requires the exact reviewed publisher digest") != 1:
        raise RuntimeError("reviewed-digest safeguard is absent or duplicated")

    path.write_text(source, encoding="utf-8", newline="\n")
    observed = digest(path)
    if not re.fullmatch(r"[0-9a-f]{64}", observed):
        raise RuntimeError("patched publisher digest is malformed")
    print(observed)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
