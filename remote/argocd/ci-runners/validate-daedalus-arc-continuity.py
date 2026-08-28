#!/usr/bin/env python3
"""Credential-free static contracts for Daedalus AWS/Hetzner ARC continuity."""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
ARC_VERSION = "0.14.2"
RUNNER_VERSION = "2.334.0"
BASE = ROOT / "remote/argocd/ci-runners/daedalus-fab/base"
DAEDALUS = ROOT / "remote/argocd/ci-runners/daedalus-fab"
AWS = ROOT / "remote/argocd/clusters/aws/daedalus-ci.applications.yaml"
HETZNER = ROOT / "remote/argocd/clusters/hetzner/daedalus-ci.applications.yaml"
AWS_KUSTOMIZATION = ROOT / "remote/argocd/clusters/aws/kustomization.yaml"
HETZNER_KUSTOMIZATION = ROOT / "remote/argocd/clusters/hetzner/kustomization.yaml"
STATUS = ROOT / "scripts/ops/gha_continuity_status.py"
STATUS_TEST = ROOT / "scripts/ops/test_gha_continuity_status.py"
WORKFLOW = ROOT / ".github/workflows/daedalus-arc-continuity.yml"
DOC = ROOT / "docs/operations/daedalus-github-actions-continuity.md"


def fail(message: str) -> None:
    print(f"ERROR: {message}", file=sys.stderr)
    raise SystemExit(1)


def require(condition: bool, message: str) -> None:
    if not condition:
        fail(message)


def read(path: Path) -> str:
    require(path.is_file(), f"missing file: {path.relative_to(ROOT)}")
    return path.read_text(encoding="utf-8")


def documents(text: str) -> list[str]:
    return [part.strip() for part in re.split(r"(?m)^---\s*$", text) if part.strip()]


def app_document(text: str, name: str) -> str:
    matches = [
        document
        for document in documents(text)
        if re.search(rf"(?m)^\s*name:\s*{re.escape(name)}\s*$", document)
    ]
    require(len(matches) == 1, f"expected exactly one Argo Application {name}, found {len(matches)}")
    return matches[0]


def require_tokens(text: str, tokens: tuple[str, ...], label: str) -> None:
    for token in tokens:
        require(token in text, f"{label} missing: {token}")


def reject_tokens(text: str, tokens: tuple[str, ...], label: str) -> None:
    for token in tokens:
        require(token not in text, f"{label} contains forbidden token: {token}")


def check_inventory() -> None:
    for path in (
        BASE / "kustomization.yaml",
        BASE / "namespace.yaml",
        BASE / "resource-policy.yaml",
        BASE / "runner-networkpolicy.yaml",
        DAEDALUS / "daedalus-arc-github.externalsecret.template.yaml",
        DAEDALUS / "daedalus-ci-smoke.workflow.template.yml",
        DAEDALUS / "continuity-status.example.json",
        DAEDALUS / "README.md",
        AWS,
        HETZNER,
        AWS_KUSTOMIZATION,
        HETZNER_KUSTOMIZATION,
        STATUS,
        STATUS_TEST,
        WORKFLOW,
        DOC,
    ):
        read(path)


def check_inert_prerequisites() -> None:
    base = read(BASE / "kustomization.yaml")
    require_tokens(
        base,
        ("namespace.yaml", "resource-policy.yaml", "runner-networkpolicy.yaml"),
        "Daedalus runner base",
    )
    reject_tokens(
        base,
        (
            "daedalus-arc-github.externalsecret.template.yaml",
            "daedalus-ci-smoke.workflow.template.yml",
            "continuity-status.example.json",
        ),
        "Daedalus runner base",
    )

    namespace = read(BASE / "namespace.yaml")
    require_tokens(
        namespace,
        (
            "name: arc-runners-daedalus",
            "pod-security.kubernetes.io/enforce: restricted",
        ),
        "runner namespace",
    )

    resource_policy = read(BASE / "resource-policy.yaml")
    require_tokens(
        resource_policy,
        (
            "kind: ResourceQuota",
            "kind: LimitRange",
            'pods: "12"',
            "limits.ephemeral-storage: 128Gi",
        ),
        "runner resource policy",
    )

    network = read(BASE / "runner-networkpolicy.yaml")
    require_tokens(
        network,
        (
            "dd.dev/ci-runner: daedalus-ci",
            "ingress: []",
            "port: 53",
            "port: 443",
            "169.254.0.0/16",
            "10.0.0.0/8",
            "172.16.0.0/12",
            "192.168.0.0/16",
        ),
        "runner NetworkPolicy",
    )
    reject_tokens(network, ("0.0.0.0/0\n      ports: []", "port: 80"), "runner NetworkPolicy")

    secret_template = read(DAEDALUS / "daedalus-arc-github.externalsecret.template.yaml")
    require_tokens(
        secret_template,
        (
            "TEMPLATE ONLY",
            "name: daedalus-fab-arc-github",
            "key: dd/ci/github-apps/daedalus-fab-arc",
            "secretKey: github_app_id",
            "secretKey: github_app_installation_id",
            "secretKey: github_app_private_key",
            "deletionPolicy: Retain",
        ),
        "ARC credential template",
    )
    reject_tokens(
        secret_template,
        ("github_token", "personal_access_token", "ghp_", "BEGIN PRIVATE KEY"),
        "ARC credential template",
    )


def check_cloud(cloud: str, path: Path, runner_group: str) -> None:
    text = read(path)
    prereqs = app_document(text, "dd-daedalus-ci-runner-prereqs")
    controller = app_document(text, "dd-daedalus-ci-arc-controller")
    runner = app_document(text, "dd-daedalus-ci-runner-set")

    require("targetRevision: dev" in prereqs, f"{cloud} prerequisites must track dev")
    require("automated:" in prereqs, f"{cloud} prerequisites must reconcile automatically")
    require("automated:" not in controller, f"{cloud} ARC controller must remain manual")
    require("automated:" not in runner, f"{cloud} ARC runner set must remain manual")

    require_tokens(
        controller,
        (
            f"targetRevision: {ARC_VERSION}",
            "chart: gha-runner-scale-set-controller",
            "dd.dev/activation-state: controller-crd-and-credential-gated",
            f"dd.dev/cloud-provider: {cloud}",
            "watchSingleNamespace: arc-runners-daedalus",
            "runAsNonRoot: true",
            "allowPrivilegeEscalation: false",
            "readOnlyRootFilesystem: true",
            'drop: ["ALL"]',
        ),
        f"{cloud} ARC controller",
    )

    require_tokens(
        runner,
        (
            f"targetRevision: {ARC_VERSION}",
            "chart: gha-runner-scale-set",
            "githubConfigUrl: https://github.com/daedalus-fab",
            "githubConfigSecret: daedalus-fab-arc-github",
            f'runnerGroup: "{runner_group}"',
            "runnerScaleSetName: daedalus-ci",
            "minRunners: 0",
            "maxRunners: 4",
            f"image: ghcr.io/actions/actions-runner:{RUNNER_VERSION}",
            "automountServiceAccountToken: false",
            "restartPolicy: Never",
            "runAsNonRoot: true",
            "runAsUser: 1001",
            "runAsGroup: 1001",
            "allowPrivilegeEscalation: false",
            'drop: ["ALL"]',
            f"value: {cloud}",
            "dd.dev/activation-state: runner-group-digest-and-smoke-gated",
        ),
        f"{cloud} ARC runner set",
    )
    reject_tokens(
        runner,
        (
            "hostPath:",
            "docker.sock",
            "containerd.sock",
            "privileged: true",
            "github_token:",
            "BEGIN PRIVATE KEY",
            "minRunners: 1",
        ),
        f"{cloud} ARC runner set",
    )


def check_cloud_isolation_and_overlay_wiring() -> None:
    check_cloud("aws", AWS, "daedalus-aws")
    check_cloud("hetzner", HETZNER, "daedalus-hetzner")

    aws = read(AWS)
    hetzner = read(HETZNER)
    require('runnerGroup: "daedalus-hetzner"' not in aws, "AWS cannot use the Hetzner runner group")
    require('runnerGroup: "daedalus-aws"' not in hetzner, "Hetzner cannot use the AWS runner group")
    require("value: hetzner" not in aws, "AWS cannot claim Hetzner provider evidence")
    require("value: aws" not in hetzner, "Hetzner cannot claim AWS provider evidence")

    for cloud, path in (("aws", AWS_KUSTOMIZATION), ("hetzner", HETZNER_KUSTOMIZATION)):
        text = read(path)
        require(
            text.count("daedalus-ci.applications.yaml") == 1,
            f"{cloud} overlay must include exactly one Daedalus continuity file",
        )


def check_smoke_and_status_contract() -> None:
    smoke = read(DAEDALUS / "daedalus-ci-smoke.workflow.template.yml")
    require_tokens(
        smoke,
        (
            "workflow_dispatch:",
            "runs-on: daedalus-ci",
            'test "$(id -u)" != 0',
            "test ! -e /var/run/docker.sock",
            "test ! -e /run/containerd/containerd.sock",
            "test ! -e /var/run/secrets/kubernetes.io/serviceaccount/token",
            "case \"$DD_CI_CLOUD\" in",
            "aws|hetzner",
            "Exercise bounded workspace lifecycle",
            "Report non-sensitive runner evidence",
        ),
        "manual runner smoke",
    )
    reject_tokens(
        smoke,
        ("pull_request:", "push:", "schedule:", "secrets.", "github.token"),
        "manual runner smoke",
    )

    status_source = read(STATUS)
    require_tokens(
        status_source,
        (
            'SCHEMA_VERSION = "gha-continuity-status.v1"',
            'EXPECTED_SCALE_SET = "daedalus-ci"',
            '"aws": "daedalus-aws"',
            '"hetzner": "daedalus-hetzner"',
            'DEFAULT_PROVIDER_ORDER = ("hetzner", "aws")',
            'choices=("arc", "build-server", "either")',
            'return 0 if result["ok"] else 2',
        ),
        "continuity status evaluator",
    )
    reject_tokens(
        status_source,
        ("subprocess", "os.system", "shell=True", "runner_label", "runs_on"),
        "continuity status evaluator",
    )

    sample = json.loads(read(DAEDALUS / "continuity-status.example.json"))
    require(sample.get("schemaVersion") == "gha-continuity-status.v1", "sample schema is wrong")
    require(
        all(
            provider.get("configured") is False
            and provider.get("registered") is False
            and provider.get("smokePassed") is False
            for provider in sample.get("arcProviders", {}).values()
        ),
        "sample ARC providers must fail closed",
    )
    completed = subprocess.run(
        [sys.executable, str(STATUS), "--snapshot", str(DAEDALUS / "continuity-status.example.json")],
        check=False,
        capture_output=True,
        text=True,
    )
    require(completed.returncode == 2, f"fail-closed sample must exit 2, got {completed.returncode}")
    result = json.loads(completed.stdout)
    require(result.get("ok") is False and result.get("failClosed") is True, "sample must report failClosed")


def check_workflow_and_docs() -> None:
    workflow = read(WORKFLOW)
    require_tokens(
        workflow,
        (
            "name: Daedalus ARC continuity",
            "permissions:\n  contents: read",
            "persist-credentials: false",
            "python3 -m unittest -v scripts/ops/test_gha_continuity_status.py",
            "python3 remote/argocd/ci-runners/validate-daedalus-arc-continuity.py",
            "kubectl kustomize remote/argocd/clusters/aws",
            "kubectl kustomize remote/argocd/clusters/hetzner",
            "docker://rhysd/actionlint@sha256:",
        ),
        "continuity workflow",
    )
    reject_tokens(
        workflow,
        ("id-token: write", "contents: write", "pull-requests: write", "secrets."),
        "continuity workflow",
    )

    docs = read(DAEDALUS / "README.md") + "\n" + read(DOC)
    for phrase in (
        "not a reimplementation of GitHub Actions",
        "daedalus-aws",
        "daedalus-hetzner",
        "minRunners: 0",
        "dd-build-server",
        "gha-clone-server-rs",
        "personal account",
        "exact remaining Actions billing balance",
        "Revoke",
        "hosted-vs-ARC parity",
    ):
        require(phrase in docs, f"continuity documentation missing phrase: {phrase}")


def main() -> int:
    check_inventory()
    check_inert_prerequisites()
    check_cloud_isolation_and_overlay_wiring()
    check_smoke_and_status_contract()
    check_workflow_and_docs()
    print("Daedalus ARC continuity contracts passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
