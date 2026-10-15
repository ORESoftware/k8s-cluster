#!/usr/bin/env python3
"""Credential-free drift contract for the DEN-2506 Meshy worker integration."""

from __future__ import annotations

import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW_PATH = ROOT / ".github/workflows/den-2506-meshy-worker-integration.yml"
SOURCE_PATH = "remote/deployments/fabrication-server-rs"
PIN_PATH = ROOT / f"{SOURCE_PATH}.source.sha"


def fail(message: str) -> None:
    raise SystemExit(f"DEN-2506 Meshy integration contract failed: {message}")


def tracked_gitlink(path: str) -> str:
    completed = subprocess.run(
        ["git", "ls-files", "--stage", "--", path],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    fields = completed.stdout.strip().split()
    if len(fields) < 4 or fields[0] != "160000":
        fail(f"{path} is not a tracked gitlink")
    return fields[1]


def main() -> None:
    if not WORKFLOW_PATH.is_file():
        fail(f"missing workflow {WORKFLOW_PATH.relative_to(ROOT)}")
    if not PIN_PATH.is_file():
        fail(f"missing reviewed source pin {PIN_PATH.relative_to(ROOT)}")

    workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
    recorded_pin = PIN_PATH.read_text(encoding="utf-8").strip()
    if not re.fullmatch(r"[0-9a-f]{40}", recorded_pin):
        fail("source sidecar must contain exactly one lowercase 40-character commit SHA")

    gitlink_pin = tracked_gitlink(SOURCE_PATH)
    if recorded_pin != gitlink_pin:
        fail(
            "reviewed source sidecar does not match the fabrication-server gitlink "
            f"({recorded_pin} != {gitlink_pin})"
        )

    required = [
        "source-contract:",
        "needs: source-contract",
        "persist-credentials: false",
        "K8S_SUBMODULE_APP_ID",
        "K8S_SUBMODULE_APP_PRIVATE_KEY",
        "init-submodules-with-github-app.sh",
        f"{SOURCE_PATH}.source.sha",
        "scripts/ci/check_meshy_integration.py",
        "scripts/ci/check_meshy_r2_archive.py",
        "crates/meshy-job/Cargo.toml",
        "crates/meshy-r2-archive/Cargo.toml",
        "crates/meshy-worker/Cargo.toml",
        "./prepare-lock.sh",
        "crates/meshy-worker/Dockerfile",
        "cargo check --locked",
        "cargo test --locked",
        "10001:10001",
        "/usr/local/bin/meshy-job-worker",
    ]
    missing = [token for token in required if token not in workflow]
    if missing:
        fail("workflow is missing required contracts: " + ", ".join(missing))

    referenced_secrets = set(re.findall(r"secrets\.([A-Z][A-Z0-9_]*)", workflow))
    expected_secrets = {
        "K8S_SUBMODULE_APP_ID",
        "K8S_SUBMODULE_APP_PRIVATE_KEY",
    }
    if referenced_secrets != expected_secrets:
        fail(
            "workflow secret surface must be exactly the owner-scoped GitHub App pair; "
            f"found {sorted(referenced_secrets)}"
        )

    forbidden = [
        "pull_request_target",
        "persist-credentials: true",
        "K8S_LIBS_DEPLOY_KEY",
        "GH_PAT",
        "remote/libs",
        "remote/submodules/discrete-event-system.rs",
        "--manifest-path remote/deployments/fabrication-server-rs/Cargo.toml",
        "--bin dd-fabrication-server",
        "MESHY_API_KEY",
        "CLOUDFLARE_API_TOKEN",
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "docker push",
        "kubectl ",
        "helm ",
        "argocd ",
        "wrangler ",
    ]
    present = [token for token in forbidden if token in workflow]
    if present:
        fail("workflow contains forbidden coupling or live-operation tokens: " + ", ".join(present))

    if workflow.index("docker build") > workflow.index("./prepare-lock.sh"):
        fail(
            "container verification must run before the host reconstructs Cargo.lock, "
            "so the image proves it used only the compact resolver artifact"
        )

    print(
        "DEN-2506 standalone Meshy worker contract is valid at "
        f"{recorded_pin} with no provider, R2, PAT, or shared-libs secret surface"
    )


if __name__ == "__main__":
    main()
