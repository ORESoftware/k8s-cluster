#!/usr/bin/env python3
"""Credential-free contracts for DEN-1549 Sonus Actions capacity failover."""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
ARC_VERSION = "0.14.2"
RUNNER_VERSION = "2.334.0"
AWS = ROOT / "remote/argocd/clusters/aws/gha-ci.applications.yaml"
HETZNER = ROOT / "remote/argocd/clusters/hetzner/gha-ci.applications.yaml"
BASE = ROOT / "remote/argocd/ci-runners/sonus-auris/base"
TEMPLATE = (
    ROOT
    / "remote/argocd/ci-runners/sonus-auris/sonus-ci-runner-set.application.template.yaml"
)
CONTROLLER = ROOT / "remote/argocd/apps/canonical-ci-arc-controller.application.yaml"
RUNNER_IMAGE = ROOT / "remote/deployments/sonus-auris-ci-runner/Dockerfile"
AUDIT = (
    ROOT
    / "remote/deployments/gha-clone-server-rs/src/bin/gha-capacity-audit.rs"
)
BROKER_DOCKERFILE = ROOT / "remote/deployments/gha-clone-server-rs/Dockerfile"
CRONJOB = ROOT / "remote/argocd/dd-next-runtime/dd-gha-clone-server.configmap.yaml"
DOC = ROOT / "docs/github-actions-self-hosted-failover.md"
WORKFLOW = ROOT / ".github/workflows/sonus-arc-scaffold.yml"
ACTIONLINT = ROOT / ".github/actionlint.yaml"


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


def application_document(text: str, name: str) -> str:
    for document in documents(text):
        if re.search(rf"(?m)^\s*name:\s*{re.escape(name)}\s*$", document):
            return document
    fail(f"missing Argo Application {name}")


def check_files() -> None:
    for path in (
        AWS,
        HETZNER,
        CONTROLLER,
        TEMPLATE,
        RUNNER_IMAGE,
        AUDIT,
        BROKER_DOCKERFILE,
        CRONJOB,
        DOC,
        WORKFLOW,
        ACTIONLINT,
        BASE / "kustomization.yaml",
        BASE / "namespace.yaml",
        BASE / "externalsecret.yaml",
        BASE / "resource-policy.yaml",
        BASE / "runner-networkpolicy.yaml",
    ):
        read(path)


def check_shared_controller() -> None:
    controller = read(CONTROLLER)
    require(
        f"targetRevision: {ARC_VERSION}" in controller,
        f"shared ARC controller must pin {ARC_VERSION}",
    )
    require(
        "releaseName: canonical-ci" in controller,
        "shared controller Helm release name drifted",
    )
    require("automated:" in controller, "shared controller must remain GitOps-managed")


def check_provider(provider: str, path: Path, group: str) -> None:
    text = read(path)
    prereq_name = f"dd-sonus-ci-runner-prereqs-{provider}"
    runner_name = f"dd-sonus-ci-runner-set-{provider}"
    prereq = application_document(text, prereq_name)
    runner = application_document(text, runner_name)

    require("automated:" in prereq, f"{provider} prerequisites must self-heal")
    require(
        "path: remote/argocd/ci-runners/sonus-auris/base" in prereq,
        f"{provider} prerequisites must use the shared base",
    )
    require(
        "automated:" not in runner,
        f"{provider} scale set must remain manual until smoke evidence exists",
    )
    for token in (
        f"targetRevision: {ARC_VERSION}",
        "githubConfigUrl: https://github.com/sonus-auris",
        "githubConfigSecret: sonus-auris-arc-github",
        f'runnerGroup: "{group}"',
        "runnerScaleSetName: sonus-ci",
        "minRunners: 0",
        "maxRunners: 4",
        "namespace: arc-systems",
        "name: canonical-ci-gha-rs-controller",
        f"image: ghcr.io/actions/actions-runner:{RUNNER_VERSION}",
        "automountServiceAccountToken: false",
        "runAsNonRoot: true",
        "allowPrivilegeEscalation: false",
        'drop: ["ALL"]',
        "seccompProfile:",
        "emptyDir:",
        "sonus-auris.dev/activation-state: credential-runner-group-and-smoke-gated",
    ):
        require(token in runner, f"{provider} runner missing contract: {token}")
    for forbidden in (
        "hostPath:",
        "docker.sock",
        "containerd.sock",
        "privileged: true",
        "BEGIN PRIVATE KEY",
        "ghp_",
        "github_pat_",
    ):
        require(forbidden not in runner, f"{provider} runner contains forbidden token: {forbidden}")


def check_active_active() -> None:
    check_provider("aws", AWS, "sonus-aws")
    check_provider("hetzner", HETZNER, "sonus-hetzner")
    require('runnerGroup: "sonus-aws"' not in read(HETZNER), "Hetzner uses AWS group")
    require('runnerGroup: "sonus-hetzner"' not in read(AWS), "AWS uses Hetzner group")
    for provider in ("aws", "hetzner"):
        kustomization = read(ROOT / f"remote/argocd/clusters/{provider}/kustomization.yaml")
        require(
            "- gha-ci.applications.yaml" in kustomization,
            f"{provider} cluster does not include the failover Applications",
        )


def check_inert_template() -> None:
    template = read(TEMPLATE)
    for token in (
        f"targetRevision: {ARC_VERSION}",
        'runnerGroup: "REPLACE_RUNNER_GROUP"',
        "runnerScaleSetName: sonus-ci",
        "name: canonical-ci-gha-rs-controller",
        "REPLACE_IMAGE_DIGEST",
    ):
        require(token in template, f"generic runner template missing: {token}")
    require("automated:" not in template, "generic runner template must stay manual")

    dockerfile = read(RUNNER_IMAGE)
    require(
        f"ARG RUNNER_VERSION={RUNNER_VERSION}" in dockerfile,
        f"custom runner must track runner {RUNNER_VERSION}",
    )
    require(
        "FROM ghcr.io/actions/actions-runner:${RUNNER_VERSION}" in dockerfile,
        "custom runner must derive from the official version-pinned runner image",
    )


def check_base_security() -> None:
    namespace = read(BASE / "namespace.yaml")
    for token in (
        "name: arc-runners-sonus",
        "pod-security.kubernetes.io/enforce: restricted",
        "pod-security.kubernetes.io/audit: restricted",
    ):
        require(token in namespace, f"runner namespace missing: {token}")

    secret = read(BASE / "externalsecret.yaml")
    for token in (
        "name: sonus-auris-arc-github",
        "name: dd-cluster-secrets",
        "key: dd/ci/github-apps/sonus-auris-arc",
        "secretKey: github_app_id",
        "secretKey: github_app_installation_id",
        "secretKey: github_app_private_key",
        "deletionPolicy: Retain",
    ):
        require(token in secret, f"runner ExternalSecret missing: {token}")

    policy = read(BASE / "runner-networkpolicy.yaml")
    for token in (
        "sonus-auris.dev/ci-runner: sonus-ci",
        "ingress: []",
        "port: 53",
        "port: 443",
        "169.254.0.0/16",
        "10.0.0.0/8",
        "172.16.0.0/12",
        "192.168.0.0/16",
    ):
        require(token in policy, f"runner NetworkPolicy missing: {token}")

    resources = read(BASE / "resource-policy.yaml")
    for token in ("kind: ResourceQuota", "kind: LimitRange", 'pods: "20"'):
        require(token in resources, f"runner resource policy missing: {token}")


def check_capacity_audit() -> None:
    audit = read(AUDIT)
    for token in (
        "/organizations/{}/settings/billing/usage",
        "/organizations/{}/settings/billing/budgets",
        "/orgs/{}/actions/variables/{}",
        "GHA_CAPACITY_INCLUDED_MINUTES",
        "GHA_CAPACITY_MUTATION_ENABLED",
        "GHA_CAPACITY_SELECTED_REPOSITORY_IDS",
        "CI_EXECUTION_MODE",
        "CI_LINUX_RUNS_ON_JSON",
        "routing variables are not mutated while capacity state is unknown",
        "routes_to_self_hosted_at_ninety_percent",
        "zero_blocking_budget_is_blocked",
        "repository_budget_uses_repository_amount",
        "missing_allowance_and_budget_is_unknown",
        "mutations_are_selected_repository_only",
    ):
        require(token in audit, f"capacity audit missing: {token}")
    for forbidden in (
        "std::process::Command",
        "tokio::process::Command",
        "Command::new",
        'env::var("GITHUB_TOKEN")',
        'env::var("GH_PAT")',
        "personal_access_token",
    ):
        require(forbidden not in audit, f"capacity audit violates bounded contract: {forbidden}")

    dockerfile = read(BROKER_DOCKERFILE)
    for token in (
        "FROM docker.io/library/rust:1.90-bookworm AS builder",
        "cargo build --locked --release --bins",
        "/usr/local/bin/gha-clone-server",
        "/usr/local/bin/gha-capacity-audit",
        "USER 10001:10001",
    ):
        require(token in dockerfile, f"broker image contract missing: {token}")
    require("latest" not in dockerfile.lower(), "broker image must not use latest tags")

    cronjob = read(CRONJOB)
    for token in (
        "kind: CronJob",
        "name: dd-gha-capacity-audit",
        "suspend: true",
        "GHA_CAPACITY_ORGANIZATION",
        "value: sonus-auris",
        "GHA_CAPACITY_INCLUDED_MINUTES",
        'value: "2000"',
        "GHA_CAPACITY_MUTATION_ENABLED",
        'value: "false"',
        "GHA_CAPACITY_SELECTED_REPOSITORY_IDS",
        'value: "1294558398"',
        "github_app_installation_token",
        "automountServiceAccountToken: false",
        "runAsNonRoot: true",
        "allowPrivilegeEscalation: false",
        'drop: ["ALL"]',
        "REPLACE_GHA_CLONE_SERVER_IMAGE_DIGEST",
        'command: ["/usr/local/bin/gha-capacity-audit"]',
        "readOnlyRootFilesystem: true",
    ):
        require(token in cronjob, f"capacity CronJob missing: {token}")
    for forbidden in ("git clone", "cargo run", "workingDir: /workspace/repo"):
        require(forbidden not in cronjob, f"capacity CronJob executes mutable source: {forbidden}")
    kustomization = read(ROOT / "remote/argocd/dd-next-runtime/kustomization.yaml")
    require(
        "- dd-gha-clone-server.configmap.yaml" in kustomization,
        "capacity CronJob is not part of the rendered continuity manifest",
    )


def check_workflow_and_docs() -> None:
    workflow = read(WORKFLOW)
    for token in (
        "branches: [dev, main]",
        "validate-sonus-arc-scaffold.py",
        "validate-sonus-gha-ha.py",
        "remote/deployments/gha-clone-server-rs/**",
        "Validate AWS/Hetzner failover and capacity contract",
        "runs-on: [self-hosted, linux, sonus-ci]",
        "persist-credentials: false",
    ):
        require(token in workflow, f"Sonus CI workflow missing: {token}")
    actionlint = read(ACTIONLINT)
    require("sonus-ci" in actionlint, "actionlint does not recognize sonus-ci")

    doc = read(DOC)
    for phrase in (
        "same scale-set name",
        "sonus-aws",
        "sonus-hetzner",
        "Administration: read",
        "Variables: write",
        "GHA_CAPACITY_MUTATION_ENABLED=false",
        "public-fork pull requests",
        "Android emulator/KVM",
        "rollback",
        "revocation of any classic PAT",
    ):
        require(phrase.lower() in doc.lower(), f"failover docs missing: {phrase}")


def check_no_secret_markers() -> None:
    paths = [
        AWS,
        HETZNER,
        CONTROLLER,
        TEMPLATE,
        AUDIT,
        BROKER_DOCKERFILE,
        CRONJOB,
        DOC,
        *(path for path in BASE.rglob("*") if path.is_file()),
    ]
    combined = "\n".join(read(path) for path in paths)
    for marker in (
        "BEGIN RSA PRIVATE KEY",
        "BEGIN PRIVATE KEY",
        "ghp_",
        "github_pat_",
        "ghs_",
    ):
        require(marker not in combined, f"possible committed secret marker: {marker}")


def main() -> None:
    checks = (
        check_files,
        check_shared_controller,
        check_active_active,
        check_inert_template,
        check_base_security,
        check_capacity_audit,
        check_workflow_and_docs,
        check_no_secret_markers,
    )
    for check in checks:
        check()
        print(f"PASS: {check.__name__}")
    print(
        f"Sonus GHA failover contract is coherent: ARC {ARC_VERSION}, runner {RUNNER_VERSION}"
    )


if __name__ == "__main__":
    main()
