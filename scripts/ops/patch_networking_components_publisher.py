#!/usr/bin/env python3
from __future__ import annotations

import hashlib
import sys
from pathlib import Path

BASE_SHA256 = "3d1679596dfd85d045afd54790db88b63f061d3450156ac81a112c8d031c2120"
PATCHED_SHA256 = "6bc89f29d9d02944eb642698aebfa425debcc05bc67843142058b3ffbe418c68"

REPLACEMENTS = (
    (
        "    _, identity = api.request('GET', '/user', expected=(200,))\n"
        "    if not isinstance(identity, dict) or not identity.get('login'):\n"
        "        raise RuntimeError('repository-administration token has no authenticated identity')\n",
        "    _, installation = api.request('GET', '/installation/repositories?per_page=1', expected=(200,))\n"
        "    if not isinstance(installation, dict) or not isinstance(installation.get('total_count'), int):\n"
        "        raise RuntimeError('installation token cannot enumerate its repositories')\n",
    ),
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
    for old, new in REPLACEMENTS:
        if source.count(old) != 1:
            raise RuntimeError("publisher patch anchor is absent or ambiguous")
        source = source.replace(old, new)
    path.write_text(source, encoding="utf-8", newline="\n")
    observed = digest(path)
    if observed != PATCHED_SHA256:
        raise RuntimeError(f"patched publisher digest mismatch: {observed}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
