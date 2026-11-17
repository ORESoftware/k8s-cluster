#!/usr/bin/env python3
"""Credential-free static contracts for the inert Rust SSR ARC scaffold."""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
ARC_VERSION = "0.14.2"
RUNNER_VERSION = "2.334.0"
RUST_VERSION = "1.97.1"
CHROME_FOR_TESTING_VERSION = "152.0.7977.64"
CHROME_FOR_TESTING_SHA256 = "8b592f066af71f054aab2cc80fc26f73c775c6d44ebb99d16ade924b24756c2e"
CHROMEDRIVER_SHA256 = "2457e3d1e204ca712d650e1f13c2b524270682471e371b4750fdbe4f15c1f2dc"
BASE = ROOT / "remote/argocd/ci-runners/rust-ssr-demos/base"
SCAFFOLD = ROOT / "remote/argocd/ci-runners/rust-ssr-demos"
APPLICATIONS = SCAFFOLD / "rust-ssr-e2e-ci.applications.template.yaml"
EXTERNAL_SECRET = SCAFFOLD / "rust-ssr-arc-github.externalsecret.template.yaml"
SMOKE = SCAFFOLD / "rust-ssr-e2e-ci-smoke.workflow.template.yml"
DOCKERFILE = SCAFFOLD / "runner-image/Dockerfile"
WORKFLOW = ROOT / ".github/workflows/rust-ssr-arc-scaffold.yml"
README = SCAFFOLD / "README.md"


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


def application(text: str, name: str) -> str:
    matches = [
        document
        for document in documents(text)
        if re.search(rf"(?m)^\s*name:\s*{re.escape(name)}\s*$", document)
    ]
    require(len(matches) == 1, f"expected one Application {name}, found {len(matches)}")
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
        APPLICATIONS,
        EXTERNAL_SECRET,
        SMOKE,
        DOCKERFILE,
        SCAFFOLD / "runner-image/.dockerignore",
        WORKFLOW,
        README,
    ):
        read(path)


def check_inert_base() -> None:
    kustomization = read(BASE / "kustomization.yaml")
    require_tokens(
        kustomization,
        ("namespace.yaml", "resource-policy.yaml", "runner-networkpolicy.yaml"),
        "Rust SSR base",
    )
    reject_tokens(
        kustomization,
        (
            "externalsecret",
            "applications.template",
            "smoke.workflow",
            "runner-image",
        ),
        "Rust SSR base",
    )

    namespace = read(BASE / "namespace.yaml")
    require_tokens(
        namespace,
        (
            "name: arc-runners-rust-ssr",
            "pod-security.kubernetes.io/enforce: restricted",
        ),
        "runner namespace",
    )

    resources = read(BASE / "resource-policy.yaml")
    require_tokens(
        resources,
        (
            "kind: ResourceQuota",
            "kind: LimitRange",
            'pods: "4"',
            "limits.ephemeral-storage: 64Gi",
        ),
        "runner resource policy",
    )

    network = read(BASE / "runner-networkpolicy.yaml")
    require_tokens(
        network,
        (
            "dd.dev/ci-runner: rust-ssr-e2e-ci",
            "ingress: []",
            "port: 53",
            "port: 443",
            "10.0.0.0/8",
            "100.64.0.0/10",
            "127.0.0.0/8",
            "169.254.0.0/16",
            "172.16.0.0/12",
            "192.168.0.0/16",
        ),
        "runner NetworkPolicy",
    )
    reject_tokens(network, ("port: 80", "hostNetwork: true"), "runner NetworkPolicy")


def check_secret_boundary() -> None:
    secret = read(EXTERNAL_SECRET)
    require_tokens(
        secret,
        (
            "TEMPLATE ONLY",
            "apiVersion: external-secrets.io/v1",
            "name: rust-ssr-e2e-arc-github",
            "key: dd/ci/github-apps/rust-ssr-e2e-arc",
            "secretKey: github_app_id",
            "secretKey: github_app_installation_id",
            "secretKey: github_app_private_key",
            "deletionPolicy: Retain",
        ),
        "ARC secret template",
    )
    reject_tokens(
        secret,
        ("github_token", "personal_access_token", "ghp_", "BEGIN PRIVATE KEY"),
        "ARC secret template",
    )

    readme = read(README)
    require_tokens(
        readme,
        (
            "Administration: write",
            "Contents: read-only",
            "Never reuse either App",
            "private-fleet-certification",
            "REPLACE_IMAGE_DIGEST",
        ),
        "Rust SSR runner README",
    )


def check_applications() -> None:
    text = read(APPLICATIONS)
    prereqs = application(text, "dd-rust-ssr-e2e-ci-runner-prereqs-template")
    controller = application(text, "dd-rust-ssr-e2e-ci-arc-controller-template")
    runner = application(text, "dd-rust-ssr-e2e-ci-runner-set-template")

    for label, app in (("prerequisites", prereqs), ("controller", controller), ("runner", runner)):
        require("automated:" not in app, f"{label} Application must remain manual")
        require("dd.dev/activation-state:" in app, f"{label} Application must expose its gate")

    require_tokens(
        prereqs,
        (
            "targetRevision: dev",
            "path: remote/argocd/ci-runners/rust-ssr-demos/base",
            "namespace: arc-runners-rust-ssr",
        ),
        "prerequisite Application",
    )

    require_tokens(
        controller,
        (
            f"targetRevision: {ARC_VERSION}",
            "chart: gha-runner-scale-set-controller",
            "watchSingleNamespace: arc-runners-rust-ssr",
            "runAsNonRoot: true",
            "allowPrivilegeEscalation: false",
            "readOnlyRootFilesystem: true",
            'drop: ["ALL"]',
        ),
        "ARC controller",
    )

    require_tokens(
        runner,
        (
            f"targetRevision: {ARC_VERSION}",
            "chart: gha-runner-scale-set",
            "githubConfigUrl: https://github.com/rust-ssr-demos/rust-ssr-e2e",
            "githubConfigSecret: rust-ssr-e2e-arc-github",
            "runnerScaleSetName: rust-ssr-e2e-ci",
            "minRunners: 0",
            "maxRunners: 1",
            "automountServiceAccountToken: false",
            "restartPolicy: Never",
            "runAsNonRoot: true",
            "runAsUser: 1001",
            "runAsGroup: 1001",
            "allowPrivilegeEscalation: false",
            'drop: ["ALL"]',
            "ghcr.io/oresoftware/rust-ssr-e2e-ci-runner@sha256:REPLACE_IMAGE_DIGEST",
            "PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD",
        ),
        "ARC runner set",
    )
    reject_tokens(
        runner,
        (
            "github.com/rust-ssr-demos\n",
            "runnerGroup:",
            "hostPath:",
            "docker.sock",
            "containerd.sock",
            "privileged: true",
            "github_token:",
            "BEGIN PRIVATE KEY",
            "minRunners: 1",
            "automated:",
        ),
        "ARC runner set",
    )


def check_runner_image() -> None:
    dockerfile = read(DOCKERFILE)
    require_tokens(
        dockerfile,
        (
            "docker.io/library/rust@sha256:408fe88047cef61a2087653b0c5255fa51c0f2d6d94ddedd7a2562a9b91a46f6",
            "ghcr.io/actions/actions-runner@sha256:91a87fc7ecea714252b01a1a3f0c64d0f8fe6a05fd466a631b9a51f88f4c7aca",
            "/usr/local/cargo",
            "/usr/local/rustup",
            "/usr/local/bin/chromium",
            "/usr/local/bin/chromedriver",
            "chrome-for-testing-public/${CHROME_FOR_TESTING_VERSION}/linux64/chrome-linux64.zip",
            "chrome-for-testing-public/${CHROME_FOR_TESTING_VERSION}/linux64/chromedriver-linux64.zip",
            CHROME_FOR_TESTING_SHA256,
            CHROMEDRIVER_SHA256,
            "sha256sum --check --strict",
            "build-essential",
            "libssl-dev",
            "pkg-config",
            "USER runner",
            'test "$(id -u)" = 1001',
        ),
        "runner Dockerfile",
    )
    require(RUNNER_VERSION in read(README), "README must record the pinned Actions runner version")
    require(RUST_VERSION in read(README), "README must record the pinned Rust version")
    require(
        CHROME_FOR_TESTING_VERSION in read(README),
        "README must record the pinned Chrome for Testing version",
    )
    for line in dockerfile.splitlines():
        if line.startswith("FROM "):
            require("@sha256:" in line, f"Dockerfile base must be digest-pinned: {line}")
    reject_tokens(
        dockerfile,
        (
            ":latest",
            "curl |",
            "curl -s |",
            "curl -fsSL |",
            "sudo ",
            "--privileged",
            "snapd",
            "chromium-browser",
            "        chromium \\",
        ),
        "runner Dockerfile",
    )


def check_smoke_and_workflow() -> None:
    smoke = read(SMOKE)
    require_tokens(
        smoke,
        (
            "workflow_dispatch:",
            "runs-on: rust-ssr-e2e-ci",
            'test "$(id -u)" != 0',
            "test ! -e /var/run/docker.sock",
            "test ! -e /run/containerd/containerd.sock",
            "test ! -e /var/run/secrets/kubernetes.io/serviceaccount/token",
            "command -v chromium",
            "command -v chromedriver",
            "command -v cargo",
            "command -v rustup",
        ),
        "smoke workflow",
    )
    reject_tokens(smoke, ("pull_request:", "push:", "schedule:", "secrets."), "smoke workflow")

    workflow = read(WORKFLOW)
    require_tokens(
        workflow,
        (
            "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
            "persist-credentials: false",
            "python3 -m py_compile",
            "validate-rust-ssr-arc-scaffold.py",
            "docker build",
            "--platform linux/amd64",
        ),
        "scaffold workflow",
    )
    reject_tokens(workflow, ("packages: write", "docker push", "secrets."), "scaffold workflow")


def main() -> None:
    check_inventory()
    check_inert_base()
    check_secret_boundary()
    check_applications()
    check_runner_image()
    check_smoke_and_workflow()
    print("rust-ssr ARC scaffold contract: passed")


if __name__ == "__main__":
    main()
