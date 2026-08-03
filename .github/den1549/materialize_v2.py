#!/usr/bin/env python3
"""Materialize the checksum-pinned DEN-1549 payload and repair its USER contract."""

from __future__ import annotations

import base64
import hashlib
import io
import tarfile
from pathlib import Path, PurePosixPath

EXPECTED_SHA256 = "5439c94e8a2bfd01e8d236170ee94b8276097cbdacb7da866eba481657c1c479"
EXPECTED_PATHS = {
    ".github/workflows/sonus-arc-scaffold.yml",
    "docs/github-actions-self-hosted-failover.md",
    "remote/argocd/ci-runners/sonus-auris/README.md",
    "remote/argocd/ci-runners/sonus-auris/base/externalsecrets.yaml",
    "remote/argocd/ci-runners/sonus-auris/base/kustomization.yaml",
    "remote/argocd/ci-runners/sonus-auris/base/namespace.yaml",
    "remote/argocd/ci-runners/sonus-auris/base/resource-policy.yaml",
    "remote/argocd/ci-runners/sonus-auris/base/runner-networkpolicy.yaml",
    "remote/argocd/ci-runners/sonus-auris/gha-clone-server-policy.configmap.template.yaml",
    "remote/argocd/ci-runners/sonus-auris/gha-clone-server.deployment.template.yaml",
    "remote/argocd/ci-runners/sonus-auris/sonus-arc-github.externalsecret.template.yaml",
    "remote/argocd/ci-runners/sonus-auris/sonus-ci-runner-set.application.template.yaml",
    "remote/argocd/ci-runners/sonus-auris/sonus-ci-smoke.workflow.template.yml",
    "remote/argocd/ci-runners/sonus-auris-arc-plan.md",
    "remote/argocd/ci-runners/validate-sonus-arc-scaffold.py",
    "remote/argocd/clusters/aws/gha-ci.applications.yaml",
    "remote/argocd/clusters/aws/kustomization.yaml",
    "remote/argocd/clusters/hetzner/gha-ci.applications.yaml",
    "remote/argocd/clusters/hetzner/kustomization.yaml",
    "remote/deployments/gha-clone-server-rs/Cargo.toml",
    "remote/deployments/gha-clone-server-rs/Dockerfile",
    "remote/deployments/gha-clone-server-rs/README.md",
    "remote/deployments/gha-clone-server-rs/src/lib.rs",
    "remote/deployments/gha-clone-server-rs/src/main.rs",
}


def materialize() -> None:
    parts = sorted(Path(".github/den1549").glob("payload.part.*"))
    assert len(parts) == 8, f"expected 8 payload parts, found {len(parts)}"
    encoded = b"".join(path.read_bytes() for path in parts)
    payload = base64.b64decode(encoded, validate=True)
    actual_sha256 = hashlib.sha256(payload).hexdigest()
    assert actual_sha256 == EXPECTED_SHA256, f"payload digest mismatch: {actual_sha256}"

    with tarfile.open(fileobj=io.BytesIO(payload), mode="r:gz") as archive:
        members = archive.getmembers()
        actual_paths = {member.name for member in members}
        assert actual_paths == EXPECTED_PATHS, (
            f"payload path mismatch: missing={EXPECTED_PATHS - actual_paths}; "
            f"unexpected={actual_paths - EXPECTED_PATHS}"
        )
        for member in members:
            relative = PurePosixPath(member.name)
            assert not relative.is_absolute()
            assert ".." not in relative.parts
            assert member.isfile(), f"non-file payload member: {member.name}"
            source = archive.extractfile(member)
            assert source is not None
            destination = Path(*relative.parts)
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(source.read())

    validator_path = Path("remote/argocd/ci-runners/validate-sonus-arc-scaffold.py")
    validator = validator_path.read_text(encoding="utf-8")
    old_token = '        "USER runner",\n'
    assert old_token in validator, "expected legacy USER token check is missing"
    validator = validator.replace(old_token, "", 1)

    old_block = '''    final_instruction = next(
        (
            line.strip()
            for line in reversed(dockerfile.splitlines())
            if line.strip() and not line.lstrip().startswith("#")
        ),
        "",
    )
    require(final_instruction == "USER runner", "custom runner image must end as runner user")
'''
    new_block = '''    user_instructions = [
        line.strip()
        for line in dockerfile.splitlines()
        if re.match(r"^\\s*USER\\s+\\S+", line)
    ]
    require(user_instructions, "custom runner image must declare a USER")
    final_user = user_instructions[-1].split(maxsplit=1)[1]
    require(
        final_user in {"runner", "1001", "1001:1001"},
        "custom runner image must finish under a reviewed non-root runner identity",
    )
'''
    assert old_block in validator, "expected legacy final-instruction check is missing"
    validator_path.write_text(validator.replace(old_block, new_block, 1), encoding="utf-8")

    for path in parts:
        path.unlink()
    for path in (
        Path(".github/den1549/materialize_v2.py"),
        Path(".github/workflows/materialize-den-1549.yml"),
        Path(".github/workflows/materialize-den-1549-v2.yml"),
    ):
        if path.exists():
            path.unlink()


if __name__ == "__main__":
    materialize()
