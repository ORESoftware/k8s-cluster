#!/usr/bin/env python3
"""Verify the human portfolio inventory is derived from `.gitmodules`."""

from __future__ import annotations

import configparser
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
GITMODULES = ROOT / ".gitmodules"
README = ROOT / "README.md"


def fail(message: str) -> None:
    raise SystemExit(f"portfolio inventory drift: {message}")


def main() -> None:
    parser = configparser.ConfigParser()
    parser.read(GITMODULES, encoding="utf-8")

    paths: list[str] = []
    urls: list[str] = []
    for section in parser.sections():
        if not section.startswith('submodule "'):
            fail(f"unexpected .gitmodules section {section!r}")
        path = parser.get(section, "path", fallback="").strip()
        url = parser.get(section, "url", fallback="").strip()
        if not path.startswith("apps/") or path.count("/") != 1:
            fail(f"submodule path must be one direct apps/ child: {path!r}")
        if not url.startswith("https://github.com/zed-pkg/") or not url.endswith(".git"):
            fail(f"submodule URL is outside the canonical zed-pkg org: {url!r}")
        paths.append(path.removeprefix("apps/"))
        urls.append(url)

    if len(paths) != len(set(paths)):
        fail(f"duplicate submodule paths: {paths}")
    if len(urls) != len(set(urls)):
        fail("duplicate submodule URLs")

    readme = README.read_text(encoding="utf-8")
    documented = re.findall(r"^  ([A-Za-z0-9._-]+)/\s+", readme, flags=re.MULTILINE)
    if len(documented) != len(set(documented)):
        fail(f"duplicate README inventory rows: {documented}")

    actual = set(paths)
    claimed = set(documented)
    if claimed != actual:
        missing = sorted(actual - claimed)
        extra = sorted(claimed - actual)
        fail(f"README/.gitmodules mismatch; missing={missing}, extra={extra}")

    stale_claims = ("SDKs (rust/ts/python/go)", "SDKs for Rust, TypeScript, Python, and Go")
    for claim in stale_claims:
        if claim in readme:
            fail(f"stale SDK claim returned: {claim!r}")
    if "ten SDKs: Rust/WASM/TypeScript/Python/Go/Dart/Gleam/Erlang/Java/Swift" not in readme:
        fail("README must name the reviewed ten-language SDK matrix")

    print(f"zed-monorepo inventory matches {len(paths)} exact gitlinks")


if __name__ == "__main__":
    main()
