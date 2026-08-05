#!/usr/bin/env python3
"""Enforce the monorepo's Zed-package and git-submodule ownership boundary."""

from __future__ import annotations

import configparser
import re
import subprocess
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
GITMODULES = ROOT / ".gitmodules"
README = ROOT / "README.md"
MANIFEST = ROOT / ".zpkg.toml"
LOCK = ROOT / ".zpkg.lock"

EXPECTED = {
    "zed-interfaces",
    "zed-api-server.rs",
    "zed-web-server.rs",
    "zed-clients",
    "zed-sync",
    "zed-docs",
    "zed-pkg.github.io",
    "zed-e2e",
}
FORBIDDEN = {"zed-cli", "zed-infra"}


def fail(message: str) -> None:
    raise SystemExit(f"portfolio inventory drift: {message}")


def load_submodules() -> dict[str, str]:
    parser = configparser.ConfigParser(interpolation=None)
    if not parser.read(GITMODULES, encoding="utf-8"):
        fail(".gitmodules is missing or unreadable")

    modules: dict[str, str] = {}
    for section in parser.sections():
        if not section.startswith('submodule "'):
            fail(f"unexpected .gitmodules section {section!r}")
        path = parser.get(section, "path", fallback="").strip()
        url = parser.get(section, "url", fallback="").strip()
        if not path.startswith("apps/") or path.count("/") != 1:
            fail(f"submodule path must be one direct apps/ child: {path!r}")
        name = path.removeprefix("apps/")
        if name in modules:
            fail(f"duplicate submodule path: {path}")
        expected_url = f"https://github.com/zed-pkg/{name}.git"
        if url != expected_url:
            fail(f"submodule URL mismatch for {name}: {url!r}")
        modules[name] = url
    return modules


def load_gitlinks() -> dict[str, tuple[str, str]]:
    output = subprocess.check_output(
        ["git", "ls-files", "--stage", "apps"], cwd=ROOT, text=True
    )
    links: dict[str, tuple[str, str]] = {}
    for line in output.splitlines():
        mode, _sha, stage, path = line.split(maxsplit=3)
        if not path.startswith("apps/") or path.count("/") != 1:
            continue
        links[path.removeprefix("apps/")] = (mode, stage)
    return links


def check_zed_package() -> None:
    with MANIFEST.open("rb") as handle:
        manifest = tomllib.load(handle)
    with LOCK.open("rb") as handle:
        lock = tomllib.load(handle)

    package = manifest.get("package", {})
    if package.get("org") != "zed-pkg" or package.get("name") != "zed-monorepo":
        fail(".zpkg.toml package identity must be zed-pkg/zed-monorepo")
    if manifest.get("dependencies"):
        fail("submodule-owned repositories must not also be Zed dependencies")
    install = manifest.get("install", {})
    if install.get("adapter") != "none" or install.get("dir") != ".vendor/.zed":
        fail("install policy must use adapter=none and dir=.vendor/.zed")
    if manifest.get("targets", {}).get("repository", {}).get("dir") != ".":
        fail("repository target must point at the package root")
    if lock.get("version") != 1:
        fail(".zpkg.lock must use lock format version 1")


def main() -> None:
    modules = load_submodules()
    actual = set(modules)
    if actual != EXPECTED:
        fail(
            "unexpected .gitmodules inventory; "
            f"missing={sorted(EXPECTED - actual)}, extra={sorted(actual - EXPECTED)}"
        )
    if actual & FORBIDDEN:
        fail(f"CLI/infra must not be submodules: {sorted(actual & FORBIDDEN)}")

    links = load_gitlinks()
    if set(links) != EXPECTED:
        fail(
            "gitlink inventory differs from .gitmodules; "
            f"missing={sorted(EXPECTED - set(links))}, extra={sorted(set(links) - EXPECTED)}"
        )
    for name, (mode, stage) in links.items():
        if mode != "160000" or stage != "0":
            fail(f"{name} is not a stage-0 gitlink: mode={mode}, stage={stage}")

    readme = README.read_text(encoding="utf-8")
    documented = re.findall(r"^  ([A-Za-z0-9._-]+)/\s+", readme, flags=re.MULTILINE)
    if len(documented) != len(set(documented)):
        fail(f"duplicate README inventory rows: {documented}")
    if set(documented) != EXPECTED:
        fail(
            "README/.gitmodules mismatch; "
            f"missing={sorted(EXPECTED - set(documented))}, "
            f"extra={sorted(set(documented) - EXPECTED)}"
        )
    for forbidden_path in ("apps/zed-cli", "apps/zed-infra"):
        if forbidden_path in readme or forbidden_path in GITMODULES.read_text(encoding="utf-8"):
            fail(f"forbidden monorepo import returned: {forbidden_path}")
    if "fourteen maintained SDK slices" not in readme:
        fail("README must name the reviewed fourteen-slice SDK matrix")
    for runtime in ("Node.js", "Deno", "Bun", "edge"):
        if runtime not in readme:
            fail(f"README must name the TypeScript {runtime} runtime")

    check_zed_package()
    print("zed-monorepo package boundary matches 8 exact gitlinks")


if __name__ == "__main__":
    main()
