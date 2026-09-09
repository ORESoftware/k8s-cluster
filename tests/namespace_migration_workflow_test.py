#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / ".github/workflows/namespace-migration-contract.yml"


def require(text: str, needle: str, message: str) -> None:
    if needle not in text:
        raise AssertionError(message)


def main() -> None:
    text = WORKFLOW.read_text(encoding="utf-8")

    require(
        text,
        "push:\n    branches: [dev, main]",
        "namespace certification must run on both dev and main pushes",
    )
    require(text, "pull_request:", "namespace certification must run for pull requests")
    require(text, "workflow_dispatch:", "namespace certification must remain manually dispatchable")
    require(
        text,
        "ref: ${{ steps.source-ref.outputs.ref }}",
        "namespace certification must checkout the selected exact source ref",
    )
    require(
        text,
        "artifacts/namespace-migration-manifest.generated.json",
        "namespace certification must retain the rendered exact-head manifest candidate",
    )

    inventory = text.index("Generate exact-head inventory before deriving the manifest")
    render = text.index("Render deterministic manifest candidate for diagnostics")
    adversarial = text.index("Run classifier and manifest adversarial tests")
    validate = text.index("Validate deterministic plaintext migration manifest")
    if not inventory < render < adversarial < validate:
        raise AssertionError(
            "namespace workflow must inventory, render, test, then validate in deterministic order"
        )

    print("Namespace migration workflow exact-head and post-merge certification checks passed")


if __name__ == "__main__":
    main()
