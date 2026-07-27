#!/usr/bin/env python3
"""Validate the platform-owned Messaging Intel GitOps security contract.

The repository already relies on Ruby's standard-library YAML parser in CI. This
script invokes that parser and consumes JSON, avoiding an unpinned Python package
installation while keeping the semantic checks readable and testable.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
TENANT_PATH = ROOT / "remote/argocd/projects/msgint-research.tenant.yaml"
PROJECT_PATH = ROOT / "remote/argocd/projects/msgint-research.appproject.yaml"
APPLICATION_PATH = ROOT / "remote/argocd/apps/msgint-capture.application.yaml"
DOC_PATH = ROOT / "docs/messaging-intel-research-deployment.md"


class ContractError(RuntimeError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ContractError(message)


def load_yaml_stream(path: Path) -> list[dict[str, Any]]:
    require(path.is_file(), f"missing required file: {path.relative_to(ROOT)}")
    ruby = (
        'require "yaml"; require "json"; '
        "puts JSON.generate(YAML.load_stream(File.read(ARGV.fetch(0))))"
    )
    result = subprocess.run(
        ["ruby", "-e", ruby, str(path)],
        check=False,
        capture_output=True,
        text=True,
    )
    require(result.returncode == 0, f"invalid YAML in {path}: {result.stderr.strip()}")
    documents = json.loads(result.stdout)
    return [document for document in documents if isinstance(document, dict)]


def by_kind(documents: list[dict[str, Any]], kind: str) -> list[dict[str, Any]]:
    return [document for document in documents if document.get("kind") == kind]


def one_by_kind(documents: list[dict[str, Any]], kind: str) -> dict[str, Any]:
    matches = by_kind(documents, kind)
    require(len(matches) == 1, f"expected exactly one {kind}, found {len(matches)}")
    return matches[0]


def validate_tenant() -> None:
    documents = load_yaml_stream(TENANT_PATH)
    expected_kinds = {
        "Namespace",
        "ResourceQuota",
        "LimitRange",
        "ServiceAccount",
        "NetworkPolicy",
    }
    require(
        expected_kinds.issubset({document.get("kind") for document in documents}),
        "tenant manifest is missing one or more required resource kinds",
    )

    namespace = one_by_kind(documents, "Namespace")
    require(namespace.get("metadata", {}).get("name") == "msgint-research", "wrong namespace")
    labels = namespace.get("metadata", {}).get("labels", {})
    require(
        labels.get("security.oresoftware.dev/data-classification") == "restricted-research",
        "namespace must carry the restricted-research classification",
    )
    for mode in ("enforce", "audit", "warn"):
        require(
            labels.get(f"pod-security.kubernetes.io/{mode}") == "restricted",
            f"Pod Security {mode} must be restricted",
        )

    for document in documents:
        if document.get("kind") == "Namespace":
            continue
        require(
            document.get("metadata", {}).get("namespace") == "msgint-research",
            f"{document.get('kind')} must be scoped to msgint-research",
        )

    service_account = one_by_kind(documents, "ServiceAccount")
    require(
        service_account.get("metadata", {}).get("name") == "msgint-runtime",
        "runtime ServiceAccount must be msgint-runtime",
    )
    require(
        service_account.get("automountServiceAccountToken") is False,
        "msgint-runtime must disable service-account token automount",
    )

    policies = {
        policy.get("metadata", {}).get("name"): policy
        for policy in by_kind(documents, "NetworkPolicy")
    }
    require(set(policies) == {"default-deny-all", "allow-dns-only"}, "unexpected NetworkPolicies")

    default_deny = policies["default-deny-all"].get("spec", {})
    require(default_deny.get("podSelector") == {}, "default deny must select every pod")
    require(
        set(default_deny.get("policyTypes", [])) == {"Ingress", "Egress"},
        "default deny must cover ingress and egress",
    )
    require(not default_deny.get("ingress"), "default deny must not allow ingress")
    require(not default_deny.get("egress"), "default deny must not allow egress")

    dns = policies["allow-dns-only"].get("spec", {})
    require(dns.get("podSelector") == {}, "DNS policy must select every pod")
    require(set(dns.get("policyTypes", [])) == {"Egress"}, "DNS policy must be egress-only")
    egress = dns.get("egress", [])
    require(len(egress) == 1, "DNS policy must contain exactly one egress rule")
    destinations = egress[0].get("to", [])
    require(len(destinations) == 1, "DNS policy must target exactly one selector pair")
    destination = destinations[0]
    require(
        destination.get("namespaceSelector", {}).get("matchLabels", {}).get(
            "kubernetes.io/metadata.name"
        )
        == "kube-system",
        "DNS egress must target kube-system",
    )
    require(
        destination.get("podSelector", {}).get("matchLabels", {}).get("k8s-app")
        == "kube-dns",
        "DNS egress must target kube-dns pods",
    )
    ports = {(port.get("protocol"), port.get("port")) for port in egress[0].get("ports", [])}
    require(ports == {("UDP", 53), ("TCP", 53)}, "DNS egress must allow only TCP/UDP 53")


def validate_project() -> None:
    project = one_by_kind(load_yaml_stream(PROJECT_PATH), "AppProject")
    require(project.get("metadata", {}).get("name") == "msgint-research", "wrong AppProject name")
    spec = project.get("spec", {})
    require(
        spec.get("sourceRepos") == ["git@github.com:messaging-intel/msgint-monorepo.git"],
        "AppProject source must be the private SSH Messaging Intel monorepo URL",
    )
    require(
        spec.get("destinations")
        == [{"server": "https://kubernetes.default.svc", "namespace": "msgint-research"}],
        "AppProject destination must be only msgint-research",
    )
    require(spec.get("clusterResourceWhitelist") == [], "cluster-scoped resources must be forbidden")
    blacklist = {
        (entry.get("group", ""), entry.get("kind"))
        for entry in spec.get("namespaceResourceBlacklist", [])
    }
    required = {
        ("", "Secret"),
        ("", "ServiceAccount"),
        ("", "ResourceQuota"),
        ("", "LimitRange"),
        ("rbac.authorization.k8s.io", "Role"),
        ("rbac.authorization.k8s.io", "RoleBinding"),
    }
    require(required.issubset(blacklist), "AppProject does not blacklist all platform-owned kinds")


def validate_application() -> None:
    application = one_by_kind(load_yaml_stream(APPLICATION_PATH), "Application")
    spec = application.get("spec", {})
    require(spec.get("project") == "msgint-research", "Application must use the dedicated AppProject")
    source = spec.get("source", {})
    require(
        source.get("repoURL") == "git@github.com:messaging-intel/msgint-monorepo.git",
        "Application must use the private SSH repository URL",
    )
    require(source.get("path") == "deploy/k8s", "Application must use deploy/k8s")
    revision = str(source.get("targetRevision", ""))
    require(re.fullmatch(r"[0-9a-f]{40}", revision) is not None, "targetRevision must be a full commit SHA")
    destination = spec.get("destination", {})
    require(
        destination
        == {"server": "https://kubernetes.default.svc", "namespace": "msgint-research"},
        "Application destination must be only msgint-research",
    )
    sync_policy = spec.get("syncPolicy", {})
    require("automated" not in sync_policy, "capture Application must remain manual/inert")
    options = set(sync_policy.get("syncOptions", []))
    require("CreateNamespace=true" not in options, "platform, not the app, must create the namespace")
    require(
        {"ServerSideApply=true", "PruneLast=true"}.issubset(options),
        "required safe sync options are missing",
    )


def validate_documentation() -> None:
    require(DOC_PATH.is_file(), "missing deployment documentation")
    text = DOC_PATH.read_text(encoding="utf-8").lower()
    for phrase in ("den-32", "suspended", "do not commit secrets", "immutable commit"):
        require(phrase in text, f"deployment documentation must mention: {phrase}")


def main() -> int:
    try:
        validate_tenant()
        validate_project()
        validate_application()
        validate_documentation()
    except (ContractError, json.JSONDecodeError) as error:
        print(f"Messaging Intel GitOps validation failed: {error}", file=sys.stderr)
        return 1
    print("Messaging Intel GitOps contract is valid")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
