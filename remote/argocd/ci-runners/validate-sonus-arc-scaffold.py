#!/usr/bin/env python3
"""Credential-free validation for the inert Sonus Auris ARC scaffold."""

from __future__ import annotations

import re
import sys
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


def extract_chart_version(text: str) -> str:
    match = re.search(r"(?m)^\s*targetRevision:\s*([0-9]+\.[0-9]+\.[0-9]+)\s*$", text)
    require(match is not None, "missing pinned ARC chart targetRevision")
    return match.group(1)


def version_tuple(value: str) -> tuple[int, int, int]:
    parts = value.split(".")
    require(len(parts) == 3 and all(part.isdigit() for part in parts), f"invalid version: {value}")
    return tuple(int(part) for part in parts)  # type: ignore[return-value]


def main() -> None:
    for path in (SCAFFOLD / "README.md", TEMPLATE, RUNNER_DOCKERFILE, RUNNER_README, CONTROLLER_APP):
        require(path.is_file(), f"missing scaffold file: {path.relative_to(ROOT)}")

    template = TEMPLATE.read_text(encoding="utf-8")
    dockerfile = RUNNER_DOCKERFILE.read_text(encoding="utf-8")
    design = (SCAFFOLD / "README.md").read_text(encoding="utf-8")
    image_docs = RUNNER_README.read_text(encoding="utf-8")
    controller = CONTROLLER_APP.read_text(encoding="utf-8")

    # The scaffold must be inert until an explicit promotion PR moves a reviewed
    # copy under remote/argocd/apps and replaces every placeholder.
    require("template-only" in template, "template must declare template-only promotion state")
    require("REPLACE_IMAGE_DIGEST" in template, "template must retain an obvious image-digest placeholder")
    require("remote/argocd/apps/" in design, "design must explain explicit promotion into active apps")
    require("does not register runners" in design, "design must state that merge alone is inert")

    # Keep chart versions aligned with the existing cluster controller until a
    # dedicated upgrade changes both in one reviewed unit.
    require(
        extract_chart_version(template) == extract_chart_version(controller),
        "runner scale-set chart version must match the committed ARC controller",
    )

    required_template_tokens = (
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
    for token in required_template_tokens:
        require(token in template, f"runner template is missing safety control: {token}")

    forbidden_template_tokens = (
        "hostPath:",
        "/var/run/docker.sock",
        "privileged: true",
        "github_token:",
        "github_app_private_key:",
        "personal_access_token:",
    )
    for token in forbidden_template_tokens:
        require(token not in template, f"runner template contains forbidden capability/secret: {token}")

    base_match = re.search(
        r"(?m)^ARG RUNNER_VERSION=([0-9]+\.[0-9]+\.[0-9]+)\s*$",
        dockerfile,
    )
    require(base_match is not None, "Dockerfile must pin an exact Actions runner version")
    runner_version = base_match.group(1)
    require(version_tuple(runner_version) >= (2, 329, 0), "runner version is below GitHub's enforced minimum")
    require(
        f"FROM ghcr.io/actions/actions-runner:${{RUNNER_VERSION}}" in dockerfile,
        "Dockerfile must derive from the official version-pinned runner image",
    )
    require("USER 1001" in dockerfile, "final image must run as the non-root runner uid")
    require("latest" not in dockerfile.lower(), "Dockerfile must not use mutable latest tags")
    require("docker.sock" not in dockerfile, "non-privileged image must not reference the host Docker socket")

    for phrase in (
        "2,000 included",
        "August 1, 2026",
        "cannot execute the iOS",
        "Android emulator",
        "positive hosted Actions budget",
        "Do not migrate required checks",
    ):
        require(phrase in design or phrase in image_docs, f"missing operational limitation: {phrase}")

    # Ensure documentation does not accidentally include PEM material or likely
    # credential assignments.
    combined = "\n".join((template, dockerfile, design, image_docs))
    require("BEGIN PRIVATE KEY" not in combined, "private key material is forbidden")
    require(not re.search(r"(?im)^\s*(token|password|private_key)\s*:\s*[^<\s]", combined), "possible committed credential")

    print("Sonus Auris ARC scaffold is inert, pinned, and policy-compliant.")
    print(f"ARC chart: {extract_chart_version(template)}; runner: {runner_version}")


if __name__ == "__main__":
    main()
