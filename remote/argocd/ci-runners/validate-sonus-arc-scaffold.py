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


def require_tokens(text: str, tokens: tuple[str, ...], label: str) -> None:
    for token in tokens:
        require(token in text, f"{label} missing: {token}")


def reject_tokens(text: str, tokens: tuple[str, ...], label: str) -> None:
    for token in tokens:
        require(token not in text, f"{label} contains forbidden token: {token}")


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
        BROKER / "tests/policy_contract.rs",
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
        require("automated:" not in controller, f"{cloud} controller must be manual")
        require("automated:" not in runner, f"{cloud} scale set must be manual")
        require(
            "controller-and-crd-audit-gated" in controller,
            f"{cloud} controller lacks its activation gate",
        )
        require(
            "credential-runner-group-and-smoke-gated" in runner,
            f"{cloud} scale set lacks its activation gate",
        )

    deployment = read(SONUS / "gha-capacity-broker.deployment.template.yaml")
    require(
        "REPLACE_GHA_CAPACITY_BROKER_IMAGE_DIGEST" in deployment,
        "capacity broker deployment must remain digest-gated",
    )
    require('name: GHA_MUTATION_ENABLED\n              value: "false"' in deployment,
            "capacity mutation must default false")


def check_arc_versions_and_isolation() -> None:
    for cloud, path, group in (
        ("aws", AWS, "sonus-aws"),
        ("hetzner", HETZNER, "sonus-hetzner"),
    ):
        text = read(path)
        controller = app_document(text, "dd-sonus-ci-arc-controller")
        runner = app_document(text, "dd-sonus-ci-runner-set")
        require(f"targetRevision: {ARC_VERSION}" in controller,
                f"{cloud} controller must pin ARC {ARC_VERSION}")
        require(f"targetRevision: {ARC_VERSION}" in runner,
                f"{cloud} runner must pin ARC {ARC_VERSION}")
        require(f"image: ghcr.io/actions/actions-runner:{RUNNER_VERSION}" in runner,
                f"{cloud} runner must pin {RUNNER_VERSION}")
        require_tokens(
            runner,
            (
                "githubConfigUrl: https://github.com/sonus-auris",
                "githubConfigSecret: sonus-auris-arc-github",
                f'runnerGroup: "{group}"',
                "runnerScaleSetName: sonus-ci",
                "minRunners: 0",
                "maxRunners: 4",
                "automountServiceAccountToken: false",
                "runAsNonRoot: true",
                "allowPrivilegeEscalation: false",
                'drop: ["ALL"]',
                "emptyDir:",
            ),
            f"{cloud} scale set",
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
            ),
            f"{cloud} scale set",
        )

    require('runnerGroup: "sonus-aws"' not in read(HETZNER),
            "Hetzner cannot use the AWS runner group")
    require('runnerGroup: "sonus-hetzner"' not in read(AWS),
            "AWS cannot use the Hetzner runner group")

    dockerfile = read(RUNNER / "Dockerfile")
    require_tokens(
        dockerfile,
        (
            f"ARG RUNNER_VERSION={RUNNER_VERSION}",
            "FROM ghcr.io/actions/actions-runner:${RUNNER_VERSION}",
            "openjdk-17-jdk-headless",
            "libgtk-3-dev",
        ),
        "runner image",
    )
    reject_tokens(
        dockerfile,
        ("docker.sock", "containerd.sock", "--privileged", ":latest"),
        "runner image",
    )


def check_three_app_secrets() -> None:
    external = read(SONUS / "base/externalsecrets.yaml")
    require_tokens(
        external,
        (
            "name: sonus-auris-arc-github",
            "name: sonus-auris-gha-capacity-broker",
            "name: sonus-auris-gha-billing",
            "key: dd/ci/github-apps/sonus-auris-arc",
            "key: dd/ci/github-apps/sonus-auris-capacity-broker",
            "key: dd/ci/github-apps/sonus-auris-billing",
            "secretKey: github_app_id",
            "secretKey: github_app_installation_id",
            "secretKey: github_app_private_key",
            "property: server_auth_secret",
        ),
        "ExternalSecret contract",
    )
    reject_tokens(
        external,
        ("billing_token", "github-billing/sonus-auris", "personal_access_token"),
        "ExternalSecret contract",
    )

    deployment = read(SONUS / "gha-capacity-broker.deployment.template.yaml")
    require_tokens(
        deployment,
        (
            "GITHUB_MUTATION_APP_ID",
            "GITHUB_MUTATION_APP_INSTALLATION_ID",
            "GITHUB_MUTATION_APP_PRIVATE_KEY_PATH",
            "/var/run/gha-mutation-app/github_app_private_key",
            "GITHUB_BILLING_APP_ID",
            "GITHUB_BILLING_APP_INSTALLATION_ID",
            "GITHUB_BILLING_APP_PRIVATE_KEY_PATH",
            "/var/run/gha-billing-app/github_app_private_key",
            "secretName: sonus-auris-gha-capacity-broker",
            "secretName: sonus-auris-gha-billing",
            "name: mutation-app-key",
            "name: billing-app-key",
            "automountServiceAccountToken: false",
            "readOnlyRootFilesystem: true",
        ),
        "broker deployment",
    )
    reject_tokens(
        deployment,
        ("GITHUB_BILLING_TOKEN_PATH", "/var/run/gha-billing/token", "billing_token"),
        "broker deployment",
    )


def check_capacity_policy() -> None:
    policy = read(SONUS / "gha-capacity-broker-policy.configmap.template.yaml")
    require_tokens(
        policy,
        (
            '"includedMinutes": 2000',
            '"warnPercent": 75',
            '"selfHostedPercent": 90',
            '"hardStopPercent": 100',
            '"selfHostedReady": false',
            '"selfHostedRunsOn": ["sonus-ci"]',
            '"selectedRepositoryIds": [1294558398]',
        ),
        "capacity policy",
    )

    lib = read(BROKER / "src/lib.rs")
    integration = read(BROKER / "tests/policy_contract.rs")
    require_tokens(
        lib,
        (
            "pub gross_quantity: f64",
            "pub discount_quantity: f64",
            "pub net_quantity: f64",
            "pub fn actions_gross_minutes",
            "pub fn actions_billable_minutes",
            "self.actions_gross_minutes()",
            "CI_HOLD_RUNNER_LABEL",
            "parses_current_public_preview_summary_schema",
            "non_github_modes_publish_a_nonexistent_runner_label",
            "runner_labels_are_trimmed_unique_and_lane_distinct",
            "repository_ids_must_be_positive_and_unique",
            "rejects_non_finite_policy_numbers",
        ),
        "broker policy implementation",
    )
    reject_tokens(lib, ("pub quantity: f64", "organization_name", "repository_name"),
                  "billing summary model")
    require_tokens(
        integration,
        (
            "official_summary_json_shape_is_accepted",
            "billing_summary_uses_gross_minutes_for_capacity_and_net_for_cost",
            "threshold_boundaries_preserve_warn_route_and_hard_stop_semantics",
            "unknown_billing_never_falls_back_to_unverified_hosted_capacity",
            "invalid_label_and_repository_policies_fail_before_mutation",
        ),
        "capacity integration tests",
    )


def check_broker_authority() -> None:
    main = read(BROKER / "src/main.rs")
    require_tokens(
        main,
        (
            "/organizations/{org}/settings/billing/usage/summary",
            '("year", now.year().to_string())',
            '("month", (now.month() as u8).to_string())',
            '("product", "Actions".to_string())',
            "GITHUB_MUTATION_APP",
            "GITHUB_BILLING_APP",
            "billing_auth: GitHubAppAuth",
            "mutation_auth: GitHubAppAuth",
            "billing_auth.installation_token",
            "mutation_auth.installation_token",
            "billing and mutation GitHub Apps must use distinct App installations",
            "billing and mutation GitHub Apps must use distinct private-key files",
            "gross_actions_minutes",
            "billable_actions_minutes",
            "/orgs/{org}/actions/variables/{}",
            "arbitrary_command_execution: false",
            "github_error_summary",
            "billing_and_mutation_apps_must_be_distinct",
        ),
        "broker authority implementation",
    )
    reject_tokens(
        main,
        (
            "GITHUB_BILLING_TOKEN_PATH",
            "normalize_billing_token",
            "std::process::Command",
            "tokio::process::Command",
            "Command::new",
            "GITHUB_TOKEN",
        ),
        "broker authority implementation",
    )

    billing_block = main.split("async fn billing_usage", 1)[1].split(
        "async fn upsert_variable", 1
    )[0]
    require("billing_auth" in billing_block,
            "billing request must use the billing App")
    require("mutation_auth" not in billing_block,
            "billing request must not use the mutation App")

    mutation_block = main.split("async fn upsert_variable", 1)[1].split(
        "fn github_error_summary", 1
    )[0]
    require("mutation_auth" in mutation_block,
            "variable mutation must use the mutation App")
    require("billing_auth" not in mutation_block,
            "variable mutation must not use the billing App")


def check_runner_and_smoke() -> None:
    network = read(SONUS / "base/runner-networkpolicy.yaml")
    require_tokens(
        network,
        (
            "dd.dev/ci-runner: sonus-ci",
            "ingress: []",
            "169.254.0.0/16",
            "10.0.0.0/8",
            "172.16.0.0/12",
            "192.168.0.0/16",
            "port: 443",
            "port: 53",
        ),
        "runner NetworkPolicy",
    )

    smoke = read(SONUS / "sonus-ci-smoke.workflow.template.yml")
    require_tokens(
        smoke,
        (
            "workflow_dispatch:",
            "runs-on: sonus-ci",
            'test "$(id -u)" != 0',
            "test ! -e /var/run/docker.sock",
            "test ! -e /run/containerd/containerd.sock",
            "test ! -e /var/run/secrets/kubernetes.io/serviceaccount/token",
            "Exercise bounded workspace lifecycle",
            "Report non-sensitive runner evidence",
        ),
        "manual runner smoke",
    )
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
        "current UTC",
        "dd-build-server",
        "gha-clone-server-rs",
        "/usage/summary",
        "Administration: read",
        "grossQuantity",
        "netQuantity",
        "billing-read App",
        "capacity-mutation App",
        "ci-capacity-hold-no-runner",
    ):
        require(phrase.lower() in combined.lower(), f"documentation missing: {phrase}")


def check_secret_markers() -> None:
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
        "GH_PAT",
        "personal_access_token",
    ):
        require(forbidden not in combined, f"possible committed secret marker: {forbidden}")
    for pattern in (
        r"ghp_[A-Za-z0-9]{30,}",
        r"github_pat_[A-Za-z0-9_]{20,}",
    ):
        require(re.search(pattern, combined) is None,
                f"possible committed GitHub credential matching {pattern}")


def check_workflow() -> None:
    workflow = read(WORKFLOW)
    require_tokens(
        workflow,
        (
            "permissions:",
            "contents: read",
            "cargo fmt --manifest-path remote/deployments/gha-capacity-broker-rs/Cargo.toml -- --check",
            "cargo test --manifest-path remote/deployments/gha-capacity-broker-rs/Cargo.toml",
            "cargo clippy --manifest-path remote/deployments/gha-capacity-broker-rs/Cargo.toml",
            "validate-sonus-arc-scaffold.py",
            "self-hosted-smoke",
        ),
        "CI workflow",
    )
    reject_tokens(workflow, ("contents: write", "pull-requests: write", "GH_PAT"),
                  "CI workflow")


CHECKS = {
    "files": check_files,
    "inert": check_inert,
    "arc": check_arc_versions_and_isolation,
    "apps": check_three_app_secrets,
    "policy": check_capacity_policy,
    "authority": check_broker_authority,
    "runner": check_runner_and_smoke,
    "docs": check_docs,
    "secrets": check_secret_markers,
    "workflow": check_workflow,
}


def main() -> None:
    requested = sys.argv[1:] or list(CHECKS)
    for name in requested:
        check = CHECKS.get(name)
        if check is None:
            fail(f"unknown check {name}; expected one of {', '.join(CHECKS)}")
        check()
        print(f"PASS: {name}")
    print(
        "Sonus ARC and capacity contract is coherent: "
        f"ARC {ARC_VERSION}, runner {RUNNER_VERSION}, three distinct GitHub Apps"
    )


if __name__ == "__main__":
    main()
