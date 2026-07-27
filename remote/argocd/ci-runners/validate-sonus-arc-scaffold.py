#!/usr/bin/env python3
"""Credential-free validation for the inert Sonus Auris ARC scaffold."""

from __future__ import annotations

import re
import sys
from collections.abc import Callable
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
SCAFFOLD = ROOT / "remote/argocd/ci-runners/sonus-auris"
TEMPLATE = SCAFFOLD / "sonus-ci-runner-set.application.template.yaml"
RUNNER_DOCKERFILE = ROOT / "remote/deployments/sonus-auris-ci-runner/Dockerfile"
RUNNER_README = ROOT / "remote/deployments/sonus-auris-ci-runner/README.md"
CONTROLLER_APP = ROOT / "remote/argocd/apps/canonical-ci-arc-controller.application.yaml"


def fail(message: str) -> None:
    print(f"ERROR: {message}", file=sys.stderr)
    raise SystemExit(1)


def require(condition: bool, message: str) -> None:
    if not condition:
        fail(message)


def flattened(text: str) -> str:
    """Normalize prose whitespace without altering code/token checks."""
    return " ".join(text.split())


def read(path: Path) -> str:
    require(path.is_file(), f"missing scaffold file: {path.relative_to(ROOT)}")
    return path.read_text(encoding="utf-8")


def extract_chart_version(text: str) -> str:
    match = re.search(r"(?m)^\s*targetRevision:\s*([0-9]+\.[0-9]+\.[0-9]+)\s*$", text)
    require(match is not None, "missing pinned ARC chart targetRevision")
    return match.group(1)


def version_tuple(value: str) -> tuple[int, int, int]:
    parts = value.split(".")
    require(len(parts) == 3 and all(part.isdigit() for part in parts), f"invalid version: {value}")
    return int(parts[0]), int(parts[1]), int(parts[2])


def check_files() -> None:
    for path in (
        SCAFFOLD / "README.md",
        TEMPLATE,
        RUNNER_DOCKERFILE,
        RUNNER_README,
        CONTROLLER_APP,
    ):
        read(path)


def check_inert() -> None:
    template = read(TEMPLATE)
    design = flattened(read(SCAFFOLD / "README.md"))
    require("template-only" in template, "template must declare template-only promotion state")
    require("REPLACE_IMAGE_DIGEST" in template, "template must retain an obvious image-digest placeholder")
    require("remote/argocd/apps/" in design, "design must explain explicit promotion into active apps")
    require("does not register runners" in design, "design must state that merge alone is inert")


def check_versions() -> None:
    template_version = extract_chart_version(read(TEMPLATE))
    controller_version = extract_chart_version(read(CONTROLLER_APP))
    require(
        template_version == controller_version,
        f"runner chart {template_version} must match controller chart {controller_version}",
    )


def check_template() -> None:
    template = read(TEMPLATE)
    required_tokens = (
        "githubConfigUrl: https://github.com/sonus-auris",
        "githubConfigSecret: sonus-auris-arc-github",
        "runnerScaleSetName: sonus-ci",
        "minRunners: 0",
        "maxRunners: 2",
        "automountServiceAccountToken: false",
        "runAsNonRoot: true",
        "allowPrivilegeEscalation: false",
        'drop: ["ALL"]',
        "seccompProfile:",
        "ephemeral-storage:",
        "emptyDir:",
    )
    for token in required_tokens:
        require(token in template, f"runner template is missing safety control: {token}")

    forbidden_tokens = (
        "hostPath:",
        "/var/run/docker.sock",
        "privileged: true",
        "github_token:",
        "github_app_private_key:",
        "personal_access_token:",
    )
    for token in forbidden_tokens:
        require(token not in template, f"runner template contains forbidden capability/secret: {token}")


def check_runner_image() -> None:
    dockerfile = read(RUNNER_DOCKERFILE)
    base_match = re.search(
        r"(?m)^ARG RUNNER_VERSION=([0-9]+\.[0-9]+\.[0-9]+)\s*$",
        dockerfile,
    )
    require(base_match is not None, "Dockerfile must pin an exact Actions runner version")
    runner_version = base_match.group(1)
    require(version_tuple(runner_version) >= (2, 329, 0), "runner version is below GitHub's enforced minimum")
    require(
        "FROM ghcr.io/actions/actions-runner:${RUNNER_VERSION}" in dockerfile,
        "Dockerfile must derive from the official version-pinned runner image",
    )
    require("USER 1001" in dockerfile, "final image must run as the non-root runner uid")
    require("latest" not in dockerfile.lower(), "Dockerfile must not use mutable latest tags")
    require("docker.sock" not in dockerfile, "non-privileged image must not reference the host Docker socket")


def check_docs() -> None:
    prose = flattened(read(SCAFFOLD / "README.md") + "\n" + read(RUNNER_README))
    for phrase in (
        "2,000 included",
        "August 1, 2026",
        "cannot execute the iOS",
        "Android emulator",
        "positive hosted Actions budget",
        "Do not migrate required checks",
    ):
        require(phrase in prose, f"missing operational limitation: {phrase}")


def check_secrets() -> None:
    combined = "\n".join(
        (
            read(TEMPLATE),
            read(RUNNER_DOCKERFILE),
            read(SCAFFOLD / "README.md"),
            read(RUNNER_README),
        )
    )
    require("BEGIN PRIVATE KEY" not in combined, "private key material is forbidden")
    require(
        not re.search(r"(?im)^\s*(token|password|private_key)\s*:\s*[^<\s]", combined),
        "possible committed credential",
    )


CHECKS: dict[str, Callable[[], None]] = {
    "files": check_files,
    "inert": check_inert,
    "versions": check_versions,
    "template": check_template,
    "runner": check_runner_image,
    "docs": check_docs,
    "secrets": check_secrets,
}


def main() -> None:
    selected = sys.argv[1:] or list(CHECKS)
    unknown = [name for name in selected if name not in CHECKS]
    require(not unknown, f"unknown checks: {unknown}; choose from {sorted(CHECKS)}")
    for name in selected:
        CHECKS[name]()
        print(f"PASS: {name}")

    template_version = extract_chart_version(read(TEMPLATE))
    runner_match = re.search(
        r"(?m)^ARG RUNNER_VERSION=([0-9]+\.[0-9]+\.[0-9]+)\s*$",
        read(RUNNER_DOCKERFILE),
    )
    require(runner_match is not None, "runner version disappeared after validation")
    print("Sonus Auris ARC scaffold is inert, pinned, and policy-compliant.")
    print(f"ARC chart: {template_version}; runner: {runner_match.group(1)}")


if __name__ == "__main__":
    main()
