#!/usr/bin/env python3
"""Fail-closed semantic admission for local Kubernetes runtime profiles.

The TypeSpec and JSON Schema files are peer authorities. This verifier enforces
runtime invariants that are intentionally narrower than the shared data shape:
no cloud mutation, immutable kind node images, and one K3s cluster per VM.

Input is supplied only through LOCAL_K8S_PROFILE so this operational helper does
not introduce a second command-line flag contract.
"""

from __future__ import annotations

import json
import os
import re
import sys
from pathlib import Path
from typing import Any

PROFILE_SCHEMA = "ores.local-k8s-runtime-profile/v1"
RUNTIMES = {"colima-kind", "docker-kind", "multipass-k3s"}
ARCHITECTURES = {"arm64", "amd64"}
RESOURCE_CLASSES = {"compact", "standard", "large"}
TOPOLOGIES = {"single-node", "three-node"}
EXPECTED_KEYS = {
    "schema",
    "name",
    "runtime",
    "architecture",
    "resourceClass",
    "topology",
    "kubernetesVersion",
    "runtimeArtifact",
    "cloudMutationPolicy",
    "cloudWrites",
}
NAME_RE = re.compile(r"^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$")
KIND_VERSION_RE = re.compile(r"^v1\.[0-9]+\.[0-9]+$")
K3S_VERSION_RE = re.compile(r"^v1\.[0-9]+\.[0-9]+\+k3s[0-9]+$")
KIND_IMAGE_RE = re.compile(
    r"^kindest/node:(v1\.[0-9]+\.[0-9]+)@sha256:([0-9a-f]{64})$"
)


class ProfileError(ValueError):
    """A deterministic profile-policy rejection."""


def _require_string(profile: dict[str, Any], key: str) -> str:
    value = profile.get(key)
    if not isinstance(value, str) or not value:
        raise ProfileError(f"{key} must be a non-empty string")
    return value


def validate_profile(profile: Any) -> dict[str, Any]:
    if not isinstance(profile, dict):
        raise ProfileError("profile must be a JSON object")

    keys = set(profile)
    missing = sorted(EXPECTED_KEYS - keys)
    extra = sorted(keys - EXPECTED_KEYS)
    if missing or extra:
        raise ProfileError(f"profile keys differ: missing={missing} extra={extra}")

    if _require_string(profile, "schema") != PROFILE_SCHEMA:
        raise ProfileError("unsupported profile schema")

    name = _require_string(profile, "name")
    if NAME_RE.fullmatch(name) is None:
        raise ProfileError("name must be a lower-case DNS label of at most 63 characters")

    runtime = _require_string(profile, "runtime")
    if runtime not in RUNTIMES:
        raise ProfileError(f"unsupported runtime: {runtime}")

    architecture = _require_string(profile, "architecture")
    if architecture not in ARCHITECTURES:
        raise ProfileError(f"unsupported architecture: {architecture}")

    resource_class = _require_string(profile, "resourceClass")
    if resource_class not in RESOURCE_CLASSES:
        raise ProfileError(f"unsupported resourceClass: {resource_class}")

    topology = _require_string(profile, "topology")
    if topology not in TOPOLOGIES:
        raise ProfileError(f"unsupported topology: {topology}")

    mutation_policy = _require_string(profile, "cloudMutationPolicy")
    if mutation_policy != "denied":
        raise ProfileError("cloudMutationPolicy must be denied")
    if profile.get("cloudWrites") is not False:
        raise ProfileError("cloudWrites must be the boolean false")

    version = _require_string(profile, "kubernetesVersion")
    artifact = _require_string(profile, "runtimeArtifact")

    if runtime in {"colima-kind", "docker-kind"}:
        if KIND_VERSION_RE.fullmatch(version) is None:
            raise ProfileError("kind kubernetesVersion must be v1.x.y")
        image_match = KIND_IMAGE_RE.fullmatch(artifact)
        if image_match is None:
            raise ProfileError("kind runtimeArtifact must be a digest-pinned kindest/node image")
        if image_match.group(1) != version:
            raise ProfileError("kind node image version must equal kubernetesVersion")
    elif runtime == "multipass-k3s":
        if K3S_VERSION_RE.fullmatch(version) is None:
            raise ProfileError("multipass-k3s kubernetesVersion must be v1.x.y+k3sN")
        if artifact != version:
            raise ProfileError("multipass-k3s runtimeArtifact must equal kubernetesVersion")
        if topology != "single-node":
            raise ProfileError(
                "multipass-k3s profiles represent one independent VM cluster and must be single-node"
            )

    return profile


def load_profile(path: Path) -> dict[str, Any]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise ProfileError(f"unable to read profile: {exc.__class__.__name__}") from exc
    return validate_profile(data)


def main() -> int:
    if len(sys.argv) != 1:
        print("verify_profile.py accepts no command-line arguments; set LOCAL_K8S_PROFILE", file=sys.stderr)
        return 2

    raw_path = os.environ.get("LOCAL_K8S_PROFILE", "")
    if not raw_path:
        print("LOCAL_K8S_PROFILE is required", file=sys.stderr)
        return 2

    try:
        profile = load_profile(Path(raw_path))
    except ProfileError as exc:
        print(f"local Kubernetes profile rejected: {exc}", file=sys.stderr)
        return 2

    print(
        json.dumps(
            {
                "ok": True,
                "name": profile["name"],
                "runtime": profile["runtime"],
                "architecture": profile["architecture"],
                "resourceClass": profile["resourceClass"],
                "topology": profile["topology"],
                "kubernetesVersion": profile["kubernetesVersion"],
                "cloudWrites": False,
            },
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
