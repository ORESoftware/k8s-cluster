#!/usr/bin/env python3
"""Credential-free contracts for the inert StreemPilot ARC/capacity scaffold."""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
BASE = ROOT / "remote/argocd/ci-runners/streempilot"
EXPECTED_REPOSITORY_IDS = [1318677845, 1318677882, 1318677908, 1318678075]
ARC_VERSION = "0.14.2"
RUNNER_VERSION = "2.334.0"
CLOUD_TEMPLATES = {
    "aws": BASE / "aws.applications.template.yaml.tpl",
    "hetzner": BASE / "hetzner.applications.template.yaml.tpl",
}
GENERIC_RUNNER_TEMPLATE = (
    BASE / "streempilot-ci-runner-set.application.template.yaml.tpl"
)
CATALOG_VISIBLE_APPLICATION_PATHS = (
    BASE / "aws.applications.template.yaml",
    BASE / "hetzner.applications.template.yaml",
    BASE / "streempilot-ci-runner-set.application.template.yaml",
)


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


def named_document(text: str, name: str) -> str:
    for document in documents(text):
        if re.search(rf"(?m)^\s*name:\s*{re.escape(name)}\s*$", document):
            return document
    fail(f"missing document named {name}")


def require_tokens(text: str, tokens: tuple[str, ...], label: str) -> None:
    for token in tokens:
        require(token in text, f"{label} missing {token!r}")


def reject_tokens(text: str, tokens: tuple[str, ...], label: str) -> None:
    for token in tokens:
        require(token not in text, f"{label} contains forbidden {token!r}")


def policy_json(text: str) -> dict[str, object]:
    marker = "  policy.json: |-\n"
    require(marker in text, "policy ConfigMap is missing policy.json block")
    body = text.split(marker, 1)[1]
    body = "\n".join(line[4:] if line.startswith("    ") else line for line in body.splitlines())
    try:
        value = json.loads(body)
    except json.JSONDecodeError as error:
        fail(f"policy JSON is invalid: {error}")
    require(isinstance(value, dict), "policy JSON must be an object")
    return value


def check_files_and_catalog_boundary() -> None:
    required = (
        "README.md",
        "base/kustomization.yaml",
        "base/namespace.yaml",
        "base/externalsecrets.yaml",
        "base/resource-policy.yaml",
        "base/runner-networkpolicy.yaml",
        "gha-capacity-broker-policy.configmap.template.yaml",
        "gha-capacity-broker.deployment.template.yaml",
        "streempilot-ci-runner-set.application.template.yaml.tpl",
        "streempilot-arc-github.externalsecret.template.yaml",
        "streempilot-ci-smoke.workflow.template.yml",
        "aws.applications.template.yaml.tpl",
        "hetzner.applications.template.yaml.tpl",
    )
    for relative in required:
        read(BASE / relative)

    for path in CATALOG_VISIBLE_APPLICATION_PATHS:
        require(
            not path.exists(),
            f"inert Application template is catalog-visible: {path.relative_to(ROOT)}",
        )

    for path in BASE.rglob("*.yaml"):
        text = read(path)
        require(
            "kind: Application" not in text,
            f"catalog-visible YAML contains an inert Argo Application: {path.relative_to(ROOT)}",
        )

    for path in (*CLOUD_TEMPLATES.values(), GENERIC_RUNNER_TEMPLATE):
        require(path.name.endswith(".yaml.tpl"), f"template suffix is not inert: {path.name}")
        require("kind: Application" in read(path), f"{path.name} is not an Application template")


def check_inert_boundary() -> None:
    kustomization = read(BASE / "base/kustomization.yaml")
    require_tokens(
        kustomization,
        (
            "namespace.yaml",
            "externalsecrets.yaml",
            "resource-policy.yaml",
            "runner-networkpolicy.yaml",
        ),
        "base kustomization",
    )
    reject_tokens(
        kustomization,
        (
            ".template.yaml",
            ".yaml.tpl",
            "aws.applications",
            "hetzner.applications",
        ),
        "base kustomization",
    )

    for cloud, path in CLOUD_TEMPLATES.items():
        text = read(path)
        controller = named_document(text, "dd-streempilot-ci-arc-controller")
        runner = named_document(text, "dd-streempilot-ci-runner-set")
        require("automated:" not in controller, f"{cloud} controller cannot auto-sync")
        require("automated:" not in runner, f"{cloud} runner set cannot auto-sync")
        require("controller-and-crd-audit-gated" in controller, f"{cloud} controller lacks gate")
        require(
            "credential-runner-group-and-smoke-gated" in runner,
            f"{cloud} runner set lacks gate",
        )

    broker = read(BASE / "gha-capacity-broker.deployment.template.yaml")
    require("REPLACE_GHA_CAPACITY_BROKER_IMAGE_DIGEST" in broker, "broker image is not digest-gated")
    require(
        'name: GHA_MUTATION_ENABLED\n              value: "false"' in broker,
        "capacity mutation must default false",
    )


def check_namespace_and_network() -> None:
    namespace = read(BASE / "base/namespace.yaml")
    require_tokens(
        namespace,
        (
            "name: arc-runners-streempilot",
            "app.kubernetes.io/part-of: streempilot-ci",
            "pod-security.kubernetes.io/enforce: restricted",
            "pod-security.kubernetes.io/audit: restricted",
            "pod-security.kubernetes.io/warn: restricted",
        ),
        "runner namespace",
    )

    quota = read(BASE / "base/resource-policy.yaml")
    require_tokens(
        quota,
        (
            "kind: ResourceQuota",
            "pods: \"20\"",
            "requests.ephemeral-storage: 64Gi",
            "limits.ephemeral-storage: 160Gi",
            "kind: LimitRange",
        ),
        "resource policy",
    )

    network = read(BASE / "base/runner-networkpolicy.yaml")
    require_tokens(
        network,
        (
            "dd.dev/ci-runner: streempilot-ci",
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
    reject_tokens(
        network,
        ("0.0.0.0/0\n      ports", "ingress:\n    -"),
        "runner NetworkPolicy",
    )


def check_three_apps() -> None:
    external = read(BASE / "base/externalsecrets.yaml")
    require_tokens(
        external,
        (
            "name: streempilot-arc-github",
            "name: streempilot-gha-capacity-broker",
            "name: streempilot-gha-billing",
            "key: dd/ci/github-apps/streempilot-arc",
            "key: dd/ci/github-apps/streempilot-capacity-broker",
            "key: dd/ci/github-apps/streempilot-billing",
            "secretKey: github_app_id",
            "secretKey: github_app_installation_id",
            "secretKey: github_app_private_key",
            "property: server_auth_secret",
        ),
        "ExternalSecret contract",
    )
    reject_tokens(
        external,
        (
            "personal_access_token",
            "github_token",
            "billing_token",
            "ghp_",
            "BEGIN PRIVATE KEY",
        ),
        "ExternalSecret contract",
    )

    broker = read(BASE / "gha-capacity-broker.deployment.template.yaml")
    require_tokens(
        broker,
        (
            "GHA_ORGANIZATION\n              value: StreemPilot",
            "GITHUB_MUTATION_APP_ID",
            "GITHUB_MUTATION_APP_INSTALLATION_ID",
            "/var/run/gha-mutation-app/github_app_private_key",
            "GITHUB_BILLING_APP_ID",
            "GITHUB_BILLING_APP_INSTALLATION_ID",
            "/var/run/gha-billing-app/github_app_private_key",
            "secretName: streempilot-gha-capacity-broker",
            "secretName: streempilot-gha-billing",
            "automountServiceAccountToken: false",
            "readOnlyRootFilesystem: true",
        ),
        "capacity broker deployment",
    )
    reject_tokens(
        broker,
        ("GITHUB_TOKEN", "billing_token", "personal_access_token"),
        "capacity broker deployment",
    )


def check_policy() -> None:
    policy_text = read(BASE / "gha-capacity-broker-policy.configmap.template.yaml")
    policy = policy_json(policy_text)
    require(policy.get("selfHostedReady") is False, "selfHostedReady must default false")
    require(policy.get("preferSelfHosted") is False, "preferSelfHosted must default false")
    require(policy.get("buildServerEnabled") is True, "build-server mode should remain available")
    require(policy.get("hostedRunsOn") == ["ubuntu-latest"], "hosted label mismatch")
    require(policy.get("selfHostedRunsOn") == ["streempilot-ci"], "ARC label mismatch")
    require(
        policy.get("selectedRepositoryIds") == EXPECTED_REPOSITORY_IDS,
        "selected repository IDs do not match reviewed StreemPilot repositories",
    )
    ids = policy["selectedRepositoryIds"]
    require(all(isinstance(value, int) and value > 0 for value in ids), "repository IDs must be positive")
    require(len(set(ids)) == len(ids), "repository IDs must be unique")
    for key in ("includedMinutes", "warnPercent", "selfHostedPercent", "hardStopPercent"):
        require(isinstance(policy.get(key), int), f"{key} must be an integer")
    require(
        0 < policy["warnPercent"] < policy["selfHostedPercent"] < policy["hardStopPercent"],
        "capacity thresholds must be strictly ordered",
    )
    require("authorized organization billing summary" in policy_text, "billing placeholder warning is missing")


def check_cloud(cloud: str, group: str, other_group: str) -> None:
    text = read(CLOUD_TEMPLATES[cloud])
    controller = named_document(text, "dd-streempilot-ci-arc-controller")
    runner = named_document(text, "dd-streempilot-ci-runner-set")
    require(f"targetRevision: {ARC_VERSION}" in controller, f"{cloud} ARC controller version mismatch")
    require(f"targetRevision: {ARC_VERSION}" in runner, f"{cloud} ARC runner chart version mismatch")
    require_tokens(
        runner,
        (
            "githubConfigUrl: https://github.com/StreemPilot",
            "githubConfigSecret: streempilot-arc-github",
            f'runnerGroup: "{group}"',
            "runnerScaleSetName: streempilot-ci",
            "minRunners: 0",
            "maxRunners: 4",
            f"image: ghcr.io/actions/actions-runner:{RUNNER_VERSION}",
            "dd.dev/ci-runner: streempilot-ci",
            f"dd.dev/cloud-provider: {cloud}",
            "automountServiceAccountToken: false",
            "restartPolicy: Never",
            "runAsNonRoot: true",
            "allowPrivilegeEscalation: false",
            'drop: ["ALL"]',
            "emptyDir:",
        ),
        f"{cloud} runner set",
    )
    reject_tokens(
        runner,
        (
            other_group,
            "hostPath:",
            "docker.sock",
            "containerd.sock",
            "privileged: true",
            "github_token:",
            "BEGIN PRIVATE KEY",
            ":latest",
        ),
        f"{cloud} runner set",
    )


def check_templates_and_smoke() -> None:
    runner_template = read(GENERIC_RUNNER_TEMPLATE)
    require_tokens(
        runner_template,
        (
            "REPLACE_RUNNER_GROUP",
            "REPLACE_CLOUD_PROVIDER",
            "REPLACE_ACTIONS_RUNNER_IMAGE_DIGEST",
            "githubConfigUrl: https://github.com/StreemPilot",
            "runnerScaleSetName: streempilot-ci",
            "minRunners: 0",
            "automountServiceAccountToken: false",
        ),
        "custom runner template",
    )

    smoke = read(BASE / "streempilot-ci-smoke.workflow.template.yml")
    require_tokens(
        smoke,
        (
            "workflow_dispatch:",
            "runs-on: streempilot-ci",
            'test "$(id -u)" != 0',
            "test ! -e /var/run/docker.sock",
            "test ! -e /run/containerd/containerd.sock",
            "test ! -e /var/run/secrets/kubernetes.io/serviceaccount/token",
            "Exercise bounded workspace lifecycle",
            "Report non-sensitive runner evidence",
        ),
        "manual runner smoke",
    )
    reject_tokens(smoke, ("pull_request:", "push:", "secrets.", "self-hosted"), "manual smoke")


def check_docs_and_credentials() -> None:
    readme = read(BASE / "README.md")
    for phrase in (
        "GitHub-hosted runners",
        "Actions Runner Controller",
        "AWS",
        "Hetzner",
        "streempilot-aws",
        "streempilot-hetzner",
        "hosted-versus-ARC",
        "no Docker/containerd socket",
        "Three GitHub Apps",
        "classic PAT is not an activation credential",
        "Rollback",
    ):
        require(phrase in readme, f"README missing {phrase!r}")

    combined = "\n".join(
        path.read_text(encoding="utf-8")
        for path in BASE.rglob("*.*")
        if path.is_file()
    )
    reject_tokens(
        combined,
        (
            "ghp_",
            "github_pat_",
            "BEGIN RSA PRIVATE KEY",
            "BEGIN EC PRIVATE KEY",
            "BEGIN PRIVATE KEY",
        ),
        "StreemPilot scaffold",
    )


def main() -> None:
    check_files_and_catalog_boundary()
    check_inert_boundary()
    check_namespace_and_network()
    check_three_apps()
    check_policy()
    check_cloud("aws", "streempilot-aws", "streempilot-hetzner")
    check_cloud("hetzner", "streempilot-hetzner", "streempilot-aws")
    check_templates_and_smoke()
    check_docs_and_credentials()
    print("StreemPilot ARC/capacity scaffold contracts passed")


if __name__ == "__main__":
    main()
