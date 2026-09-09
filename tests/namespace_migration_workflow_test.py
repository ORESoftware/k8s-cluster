#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / ".github/workflows/namespace-migration-contract.yml"
CLASSIFIER = ROOT / "scripts/ci/classify_repo_check_scope.py"


def require(text: str, needle: str, message: str) -> None:
    if needle not in text:
        raise AssertionError(message)


def load_classifier():
    spec = importlib.util.spec_from_file_location("namespace_repo_check_scope", CLASSIFIER)
    if spec is None or spec.loader is None:
        raise AssertionError("unable to load repo-check scope classifier")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


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

    classifier = load_classifier()
    scope = classifier.classify(
        "pull_request",
        [
            ".github/workflows/namespace-migration-contract.yml",
            "docs/namespace-migration-manifest.md",
            "scripts/ci/classify_repo_check_scope.py",
            "tests/namespace_migration_workflow_test.py",
        ],
    )
    if scope["private_contracts_required"] is not False:
        raise AssertionError("namespace-only contract diffs must not require private gitlinks")
    if scope["credential_free_contract_only"] is not True:
        raise AssertionError("namespace-only contract diffs must classify credential-free")

    print("Namespace migration workflow and credential-free scope checks passed")


if __name__ == "__main__":
    main()
