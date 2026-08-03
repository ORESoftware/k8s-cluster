#!/usr/bin/env python3
"""Credential-free contract checks for DEN-381 and DEN-1549."""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
ARC_VERSION = "0.14.2"
RUNNER_VERSION = "2.334.0"
SONUS = ROOT / "remote/argocd/ci-runners/sonus-auris"
AWS = ROOT / "remote/argocd/clusters/aws/gha-ci.applications.yaml"
HETZNER = ROOT / "remote/argocd/clusters/hetzner/gha-ci.applications.yaml"
BROKER = ROOT / "remote/deployments/gha-capacity-broker-rs"
RUNNER = ROOT / "remote/deployments/sonus-auris-ci-runner"
WORKFLOW = ROOT / ".github/workflows/sonus-arc-scaffold.yml"
DOC = ROOT / "docs/github-actions-self-hosted-failover.md"


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
    for document in documents(text):
        if re.search(rf"(?m)^\s*name:\s*{re.escape(name)}\s*$", document):
            return document
    fail(f"missing Argo Application {name}")


def check_files() -> None:
    for path in (
        SONUS / "README.md",
        SONUS / "base/kustomization.yaml",
        SONUS / "base/namespace.yaml",
        SONUS / "base/externalsecrets.yaml",
        SONUS / "base/resource-policy.yaml",
        SONUS / "base/runner-networkpolicy.yaml",
        SONUS / "gha-capacity-broker-policy.configmap.template.yaml",
        SONUS / "gha-capacity-broker.deployment.template.yaml",
        SONUS / "sonus-ci-runner-set.application.template.yaml",
        SONUS / "sonus-arc-github.externalsecret.template.yaml",
        SONUS / "sonus-ci-smoke.workflow.template.yml",
        ROOT / "remote/argocd/ci-runners/sonus-auris-arc-plan.md",
        AWS,
        HETZNER,
        BROKER / "Cargo.toml",
        BROKER / "src/lib.rs",
        BROKER / "src/main.rs",
        BROKER / "Dockerfile",
        BROKER / "README.md",
        RUNNER / "Dockerfile",
        RUNNER / "README.md",
        WORKFLOW,
        DOC,
    ):
        read(path)


def check_inert() -> None:
    base = read(SONUS / "base/kustomization.yaml")
    for template in (
        "gha-capacity-broker-policy.configmap.template.yaml",
        "gha-capacity-broker.deployment.template.yaml",
        "sonus-ci-runner-set.application.template.yaml",
        "sonus-arc-github.externalsecret.template.yaml",
    ):
        require(template not in base, f"template must remain inactive: {template}")

    for cloud, path in (("aws", AWS), ("hetzner", HETZNER)):
        text = read(path)
        controller = app_document(text, "dd-sonus-ci-arc-controller")
        runner = app_document(text, "dd-sonus-ci-runner-set")
        require("automated:" not in controller, f"{cloud} controller must be audit-gated")
        require("automated:" not in runner, f"{cloud} scale set must be credential-gated")
        require(
            "controller-and-crd-audit-gated" in controller,
            f"{cloud} controller lacks the activation gate",
        )
        require(
            "credential-runner-group-and-smoke-gated" in runner,
            f"{cloud} scale set lacks the activation gate",
        )

    deployment = read(SONUS / "gha-capacity-broker.deployment.template.yaml")
    require(
        "REPLACE_GHA_CAPACITY_BROKER_IMAGE_DIGEST" in deployment,
        "broker deployment must remain digest-gated",
    )
    require('value: "false"' in deployment, "broker mutation must default false")


def check_versions() -> None:
    for cloud, path in (("aws", AWS), ("hetzner", HETZNER)):
        text = read(path)
        controller = app_document(text, "dd-sonus-ci-arc-controller")
        runner = app_document(text, "dd-sonus-ci-runner-set")
        require(
            f"targetRevision: {ARC_VERSION}" in controller,
            f"{cloud} controller must pin ARC {ARC_VERSION}",
        )
        require(
            f"targetRevision: {ARC_VERSION}" in runner,
            f"{cloud} runner must pin ARC {ARC_VERSION}",
        )
        require(
            f"image: ghcr.io/actions/actions-runner:{RUNNER_VERSION}" in runner,
            f"{cloud} runner must pin {RUNNER_VERSION}",
        )

    template = read(SONUS / "sonus-ci-runner-set.application.template.yaml")
    require(
        f"targetRevision: {ARC_VERSION}" in template,
        "single-cluster template must track the reviewed ARC version",
    )
    dockerfile = read(RUNNER / "Dockerfile")
    require(
        f"ARG RUNNER_VERSION={RUNNER_VERSION}" in dockerfile,
        "custom runner Dockerfile must track the reviewed runner version",
    )


def check_template() -> None:
    template = read(SONUS / "sonus-ci-runner-set.application.template.yaml")
    for token in (
        "promotion-state: template-only",
        'runnerGroup: "REPLACE_RUNNER_GROUP"',
        "REPLACE_IMAGE_DIGEST",
        "githubConfigUrl: https://github.com/sonus-auris",
        "githubConfigSecret: sonus-auris-arc-github",
        "runnerScaleSetName: sonus-ci",
        "name: sonus-ci-arc-gha-rs-controller",
        "automountServiceAccountToken: false",
        "tool-cache",
        "sizeLimit: 10Gi",
    ):
        require(token in template, f"custom-image template missing: {token}")
    require("automated:" not in template, "custom-image template must remain manual")
    for forbidden in (
        "hostPath:",
        "docker.sock",
        "containerd.sock",
        "privileged: true",
        "github_token:",
        "BEGIN PRIVATE KEY",
        ":latest",
    ):
        require(forbidden not in template, f"custom-image template contains: {forbidden}")

    aws = read(AWS)
    hetzner = read(HETZNER)
    for cloud, text, group in (
        ("aws", aws, "sonus-aws"),
        ("hetzner", hetzner, "sonus-hetzner"),
    ):
        runner = app_document(text, "dd-sonus-ci-runner-set")
        for token in (
            "githubConfigUrl: https://github.com/sonus-auris",
            "githubConfigSecret: sonus-auris-arc-github",
            f'runnerGroup: "{group}"',
            "runnerScaleSetName: sonus-ci",
            "minRunners: 0",
            "maxRunners: 4",
            "controllerServiceAccount:",
            "namespace: arc-systems",
            "name: sonus-ci-arc-gha-rs-controller",
            "automountServiceAccountToken: false",
            "runAsNonRoot: true",
            "allowPrivilegeEscalation: false",
            'drop: ["ALL"]',
            "emptyDir:",
            "sizeLimit: 20Gi",
            "sizeLimit: 4Gi",
        ):
            require(token in runner, f"{cloud} runner missing contract: {token}")
        for forbidden in (
            "hostPath:",
            "docker.sock",
            "containerd.sock",
            "privileged: true",
            "github_token:",
            "BEGIN PRIVATE KEY",
        ):
            require(forbidden not in runner, f"{cloud} runner contains forbidden token: {forbidden}")

    require('runnerGroup: "sonus-aws"' not in hetzner, "Hetzner cannot use AWS group")
    require('runnerGroup: "sonus-hetzner"' not in aws, "AWS cannot use Hetzner group")
    for cloud in ("aws", "hetzner"):
        kustomization = read(ROOT / f"remote/argocd/clusters/{cloud}/kustomization.yaml")
        require(
            "- gha-ci.applications.yaml" in kustomization,
            f"{cloud} overlay does not include CI Applications",
        )


def check_activation() -> None:
    external = read(SONUS / "base/externalsecrets.yaml")
    for token in (
        "apiVersion: external-secrets.io/v1",
        "name: sonus-auris-arc-github",
        "name: sonus-auris-gha-capacity-broker",
        "name: dd-cluster-secrets",
        "key: dd/ci/github-apps/sonus-auris-arc",
        "key: dd/ci/github-apps/sonus-auris-capacity-broker",
        "secretKey: github_app_id",
        "secretKey: github_app_installation_id",
        "secretKey: github_app_private_key",
        "property: server_auth_secret",
    ):
        require(token in external, f"ExternalSecret contract missing: {token}")

    policy = read(SONUS / "gha-capacity-broker-policy.configmap.template.yaml")
    for token in (
        '"includedMinutes": 2000',
        '"selfHostedReady": false',
        '"selfHostedRunsOn": ["sonus-ci"]',
        '"selectedRepositoryIds": [1294558398]',
    ):
        require(token in policy, f"capacity policy missing: {token}")

    deployment = read(SONUS / "gha-capacity-broker.deployment.template.yaml")
    for token in (
        "GHA_ORGANIZATION",
        "value: sonus-auris",
        "GHA_ORG_POLICY_JSON",
        "GITHUB_APP_PRIVATE_KEY_PATH",
        "automountServiceAccountToken: false",
        "readOnlyRootFilesystem: true",
    ):
        require(token in deployment, f"broker deployment missing: {token}")


def check_runner() -> None:
    network = read(SONUS / "base/runner-networkpolicy.yaml")
    for token in (
        "dd.dev/ci-runner: sonus-ci",
        "ingress: []",
        "169.254.0.0/16",
        "10.0.0.0/8",
        "172.16.0.0/12",
        "192.168.0.0/16",
        "port: 443",
        "port: 53",
    ):
        require(token in network, f"runner NetworkPolicy missing: {token}")

    dockerfile = read(RUNNER / "Dockerfile")
    for token in (
        f"ARG RUNNER_VERSION={RUNNER_VERSION}",
        "FROM ghcr.io/actions/actions-runner:${RUNNER_VERSION}",
        "openjdk-17-jdk-headless",
        "libgtk-3-dev",
    ):
        require(token in dockerfile, f"custom runner image missing: {token}")
    for forbidden in (
        "docker.sock",
        "containerd.sock",
        "--privileged",
        "github_token",
        "ghp_",
        ":latest",
    ):
        require(
            forbidden not in dockerfile,
            f"custom runner image contains forbidden token: {forbidden}",
        )
    user_instructions = [
        line.strip()
        for line in dockerfile.splitlines()
        if re.match(r"^\s*USER\s+\S+", line)
    ]
    require(user_instructions, "custom runner image must declare a USER")
    final_user = user_instructions[-1].split(maxsplit=1)[1]
    require(
        final_user in {"runner", "1001", "1001:1001"},
        "custom runner image must finish under a reviewed non-root runner identity",
    )

    smoke = read(SONUS / "sonus-ci-smoke.workflow.template.yml")
    for token in (
        "workflow_dispatch:",
        "runs-on: sonus-ci",
        'test "$(id -u)" != 0',
        "test ! -e /var/run/docker.sock",
        "test ! -e /run/containerd/containerd.sock",
        "test ! -e /var/run/secrets/kubernetes.io/serviceaccount/token",
        "test -x /usr/bin/git",
        "test -x /usr/bin/curl",
        "test -x /usr/bin/python3",
        "Exercise bounded workspace lifecycle",
        "Report non-sensitive runner evidence",
    ):
        require(token in smoke, f"smoke workflow missing: {token}")
    require("pull_request" not in smoke, "self-hosted smoke must remain manual-only")
    require("secrets." not in smoke, "self-hosted smoke must not consume repo secrets")


def check_docs() -> None:
    combined = "\n".join(
        read(path)
        for path in (
            DOC,
            SONUS / "README.md",
            ROOT / "remote/argocd/ci-runners/sonus-auris-arc-plan.md",
            BROKER / "README.md",
            RUNNER / "README.md",
        )
    )
    for phrase in (
        "not a clone of GitHub's proprietary workflow service",
        "sonus-aws",
        "sonus-hetzner",
        "selected-repository",
        "macOS",
        "Android/KVM",
        "Revoke",
        "hosted-vs-ARC parity",
        "personal GitHub account",
        "current UTC year and month",
        "dd-build-server",
    ):
        require(phrase.lower() in combined.lower(), f"documentation missing: {phrase}")


def check_secrets() -> None:
    paths = [
        *SONUS.rglob("*.*"),
        AWS,
        HETZNER,
        BROKER / "src/main.rs",
        BROKER / "README.md",
        DOC,
        WORKFLOW,
    ]
    combined = "\n".join(read(path) for path in paths if path.is_file())
    for forbidden in (
        "BEGIN RSA PRIVATE KEY",
        "BEGIN PRIVATE KEY",
        "ghp_",
        "github_pat_",
        "GH_PAT",
        "personal_access_token",
    ):
        require(forbidden not in combined, f"possible committed secret marker: {forbidden}")

    main = read(BROKER / "src/main.rs")
    lib = read(BROKER / "src/lib.rs")
    for forbidden in (
        "std::process::Command",
        "tokio::process::Command",
        "Command::new",
        "GITHUB_TOKEN",
    ):
        require(forbidden not in main, f"broker violates bounded control-plane contract: {forbidden}")
    for token in (
        "/organizations/{org}/settings/billing/usage",
        '("year", now.year().to_string())',
        '("month", (now.month() as u8).to_string())',
        "/orgs/{org}/actions/variables/{}",
        "GHA_ORGANIZATION",
        "GHA_ORG_POLICY_JSON",
        "arbitrary_command_execution: false",
    ):
        require(token in main, f"broker implementation missing: {token}")
    for token in (
        "moves_to_arc_at_threshold",
        "uses_bounded_build_server_only_at_hard_stop",
        "billing_failure_fails_closed_to_validated_arc",
        "broad_variable_visibility_is_impossible_without_repo_ids",
        "prefer_self_hosted_overrides_low_usage_after_certification",
        "prefer_self_hosted_does_not_bypass_readiness",
        "unavailable_billing_holds_before_arc_certification",
        "negative_usage_is_clamped_and_product_matching_is_case_insensitive",
        "self_hosted_label_must_be_explicit",
        "rejects_non_finite_policy_numbers",
    ):
        require(token in lib, f"broker unit test missing: {token}")


CHECKS = {
    "files": check_files,
    "inert": check_inert,
    "versions": check_versions,
    "template": check_template,
    "activation": check_activation,
    "runner": check_runner,
    "docs": check_docs,
    "secrets": check_secrets,
}


def main() -> None:
    requested = sys.argv[1:] or list(CHECKS)
    for name in requested:
        check = CHECKS.get(name)
        if check is None:
            fail(f"unknown check {name}; expected one of {', '.join(CHECKS)}")
        check()
        print(f"PASS: {name}")
    print(f"Sonus ARC failover contract is coherent: ARC {ARC_VERSION}, runner {RUNNER_VERSION}")


if __name__ == "__main__":
    main()
