#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import re
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
BRIDGE = ROOT / "remote/deployments/gha-clone-server-rs"
RUNNER = ROOT / "remote/deployments/oresoftware-ci-runner"
CI_ROOT = ROOT / "remote/argocd/ci-runners/oresoftware"
BRIDGE_MANIFEST = CI_ROOT / "bridge"
CONTROL_PLANE = CI_ROOT / "control-plane"
WORKFLOWS = ROOT / ".github/workflows"
DOC = ROOT / "docs/operations/github-actions-self-hosted-fallback.md"
REGISTER = ROOT / "scripts/ops/register_github_org_webhook.py"


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def configmap_json(path: Path, key: str) -> object:
    lines = read(path).splitlines()
    marker = f"  {key}: |"
    start = lines.index(marker) + 1
    body: list[str] = []
    for line in lines[start:]:
        if line and not line.startswith("    "):
            break
        body.append(line[4:] if line.startswith("    ") else "")
    return json.loads("\n".join(body))


def load_registrar():
    spec = importlib.util.spec_from_file_location("webhook_registrar", REGISTER)
    if spec is None or spec.loader is None:
        raise RuntimeError("unable to load webhook registrar")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class GhaCloneServerContractTests(unittest.TestCase):
    def test_rust_bridge_is_failure_only_and_fail_closed(self) -> None:
        source = "\n".join(
            read(path)
            for path in [BRIDGE / "build.rs", *sorted((BRIDGE / "src").rglob("*"))]
            if path.is_file()
        )
        cargo = read(BRIDGE / "Cargo.toml")
        for dependency in ("axum", "hmac", "sha2", "subtle", "reqwest"):
            self.assertRegex(cargo, rf"(?m)^{dependency}\s*=")
        for phrase in (
            "x-hub-signature-256",
            "Hmac::<Sha256>",
            "ConstantTimeEq",
            "FAILURE_CONCLUSIONS",
            'value != "push" && value != "workflow_dispatch"',
            "head_repository",
            "ignored fork-originated workflow_run",
            "ignored workflow_run without head repository",
            "valid_commit_sha",
            "valid_delivery",
            "workflowDispatch rules",
            "buildServerProfile rules",
            'route("/ci/github/webhook"',
            'route("/metrics"',
        ):
            with self.subTest(phrase=phrase):
                self.assertIn(phrase, source)
        self.assertNotIn('"success".to_string()', source)
        self.assertNotRegex(source, r"caller.*(?:command|script|image)")
        self.assertNotIn("Command::new", source)
        self.assertNotIn("/bin/bash", source)

    def test_static_rule_dispatches_only_trusted_push_failure(self) -> None:
        rules = configmap_json(BRIDGE_MANIFEST / "configmap.yaml", "rules.json")
        self.assertIsInstance(rules, list)
        self.assertEqual(len(rules), 1)
        rule = rules[0]
        self.assertEqual(rule["repo"], "ORESoftware/k8s-cluster")
        self.assertEqual(rule["workflow"], "repo checks")
        self.assertEqual(rule["branches"], ["main", "dev"])
        self.assertEqual(rule["sourceEvents"], ["push"])
        self.assertNotIn("success", rule["conclusions"])
        self.assertNotIn("pull_request", rule["sourceEvents"])
        action = rule["action"]
        self.assertEqual(action["kind"], "workflowDispatch")
        self.assertEqual(action["workflowFile"], "self-hosted-fallback.yml")
        self.assertEqual(action["workflowName"], "Self-hosted fallback")
        self.assertEqual(action["runner"], "oresoftware-ci")
        self.assertNotEqual(action["workflowName"].casefold(), rule["workflow"].casefold())

    def test_bridge_gitops_boundary_is_exact_and_non_privileged(self) -> None:
        deployment = read(BRIDGE_MANIFEST / "deployment.yaml")
        ingress = read(BRIDGE_MANIFEST / "ingress.yaml")
        policy = read(BRIDGE_MANIFEST / "networkpolicy.yaml")
        service = read(BRIDGE_MANIFEST / "service.yaml")
        kustomization = read(BRIDGE_MANIFEST / "kustomization.yaml")
        for filename in (
            "configmap.yaml",
            "deployment.yaml",
            "service.yaml",
            "networkpolicy.yaml",
            "ingress.yaml",
        ):
            self.assertIn(filename, kustomization)
            self.assertTrue((BRIDGE_MANIFEST / filename).is_file())
        self.assertIn("ghcr.io/oresoftware/gha-clone-server:main", deployment)
        self.assertIn("runAsNonRoot: true", deployment)
        self.assertIn('readOnlyRootFilesystem: true', deployment)
        self.assertIn('allowPrivilegeEscalation: false', deployment)
        self.assertIn('drop: ["ALL"]', deployment)
        self.assertIn("automountServiceAccountToken: false", deployment)
        self.assertIn("BUILD_SERVER_GITHUB_WEBHOOK_SECRET", deployment)
        self.assertIn("SERVER_AUTH_SECRET", deployment)
        self.assertIn("GH_PAT", deployment)
        self.assertNotRegex(deployment, r"ghp_[A-Za-z0-9]{20,}")
        self.assertIn("port: 8117", service)
        self.assertIn("path: /ci/github/webhook", ingress)
        self.assertIn("pathType: Exact", ingress)
        self.assertIn("ingress-nginx", policy)
        self.assertIn("app: dd-build-server", policy)
        self.assertIn("port: 8100", policy)
        self.assertIn("port: 443", policy)

    def test_multicluster_arc_application_set_has_explicit_activation_gate(self) -> None:
        appset = read(CONTROL_PLANE / "applicationset.yaml")
        for phrase in (
            "name: oresoftware-ci-arc-controllers",
            "chart: gha-runner-scale-set-controller",
            "name: oresoftware-ci-runners",
            "chart: gha-runner-scale-set",
            'dd.dev/managed: "true"',
            "dd.dev/ci-runners: oresoftware",
            "targetRevision: 0.14.2",
            'githubConfigUrl: "https://github.com/ORESoftware"',
            "githubConfigSecret: oresoftware-arc-github",
            "runnerScaleSetName: oresoftware-ci",
            "controllerServiceAccount:",
            "name: oresoftware-ci-gha-rs-controller",
            "runnerGroup: 'oresoftware-ci-{{index .metadata.labels",
            "dd.dev/cloud",
            "minRunners: 0",
            "maxRunners: 4",
            "automountServiceAccountToken: false",
            'drop: ["ALL"]',
            "ghcr.io/oresoftware/oresoftware-ci-runner:main",
        ):
            with self.subTest(phrase=phrase):
                self.assertIn(phrase, appset)
        self.assertNotIn("github_app_private_key:", appset)
        self.assertNotRegex(appset, r"ghp_[A-Za-z0-9]{20,}")
        self.assertIn("{{.server}}", appset)

    def test_fallback_workflow_is_manual_only_and_uses_exact_sha(self) -> None:
        workflow = read(WORKFLOWS / "self-hosted-fallback.yml")
        self.assertIn("name: Self-hosted fallback", workflow)
        trigger = workflow[workflow.index("on:") : workflow.index("permissions:")]
        self.assertIn("workflow_dispatch:", trigger)
        self.assertNotIn("pull_request:", trigger)
        self.assertNotIn("push:", trigger)
        self.assertIn("runs-on: oresoftware-ci", workflow)
        self.assertNotRegex(workflow, r"runs-on:\s*\$\{\{")
        self.assertIn("ref: ${{ inputs.source_sha }}", workflow)
        self.assertIn('test "$(git rev-parse HEAD)" = "$SOURCE_SHA"', workflow)
        self.assertIn("git merge-base --is-ancestor", workflow)
        self.assertIn('refs/remotes/origin/${SOURCE_REF}', workflow)
        self.assertIn('test "$SOURCE_REPOSITORY" = "$GITHUB_REPOSITORY"', workflow)
        self.assertIn('test "$RUNNER_LABEL" = oresoftware-ci', workflow)
        self.assertIn("persist-credentials: false", workflow)
        self.assertIn("submodules: false", workflow)
        self.assertIn("cargo test --manifest-path", workflow)

    def test_image_contract_keeps_official_runner_and_pinned_build_actions(self) -> None:
        dockerfile = read(RUNNER / "Dockerfile")
        image_workflow = read(WORKFLOWS / "gha-clone-images.yml")
        for phrase in (
            "ARG RUNNER_VERSION=2.334.0",
            "FROM ghcr.io/actions/actions-runner:${RUNNER_VERSION}",
            "build-essential",
            "git-lfs",
            "python3",
            "USER 1001",
            "WORKDIR /home/runner",
        ):
            self.assertIn(phrase, dockerfile)
        for pinned in (
            "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
            "docker/setup-buildx-action@bb05f3f5519dd87d3ba754cc423b652a5edd6d2c",
            "docker/login-action@dbcb813823bdd20940b903addbd779551569679f",
            "docker/metadata-action@dc802804100637a589fabce1cb79ff13a1411302",
            "docker/build-push-action@53b7df96c91f9c12dcc8a07bcb9ccacbed38856a",
        ):
            self.assertIn(pinned, image_workflow)
        self.assertIn("gha-clone-server", image_workflow)
        self.assertIn("oresoftware-ci-runner", image_workflow)

    def test_org_webhook_registrar_is_idempotent_and_secret_safe(self) -> None:
        registrar = load_registrar()
        payload = registrar.desired_hook_payload(
            "https://ci.example.test/ci/github/webhook",
            "x" * 32,
        )
        self.assertEqual(payload["events"], ["workflow_run"])
        self.assertTrue(payload["active"])
        self.assertEqual(payload["config"]["content_type"], "json")
        self.assertEqual(payload["config"]["insecure_ssl"], "0")
        hooks = [
            {"id": 1, "config": {"url": "https://elsewhere.invalid/hook"}},
            {"id": 2, "config": {"url": "https://ci.example.test/ci/github/webhook"}},
        ]
        self.assertEqual(
            registrar.find_hook(hooks, "https://ci.example.test/ci/github/webhook")["id"],
            2,
        )
        source = read(REGISTER)
        self.assertIn("/orgs/{settings.org}/hooks", source)
        self.assertIn('"PATCH"', source)
        self.assertIn('"POST"', source)
        self.assertNotIn("print(payload", source)
        self.assertNotIn("print(settings", source)
        self.assertNotIn("error.read", source)

    def test_documented_rollout_preserves_gitops_and_credential_boundaries(self) -> None:
        doc = read(DOC)
        runner_doc = read(CI_ROOT / "README.md")
        for phrase in (
            "Use GitHub's official Actions Runner Controller",
            "dd-build-server",
            "oresoftware-ci-aws",
            "oresoftware-ci-hetzner",
            "dd.dev/ci-runners=oresoftware",
            "immutable digests",
            "register_github_org_webhook.py --dry-run",
            "Never reuse a token",
            "single-replica with in-memory delivery",
            "not a guaranteed substitute",
            "Rollback",
        ):
            with self.subTest(phrase=phrase):
                self.assertIn(phrase, doc)
        self.assertIn("produces **zero child Applications**", runner_doc)
        self.assertIn("GitOps control plane", runner_doc)

    def test_new_files_contain_no_plaintext_github_token(self) -> None:
        paths = [
            *BRIDGE.rglob("*"),
            *RUNNER.rglob("*"),
            *CI_ROOT.rglob("*"),
            WORKFLOWS / "self-hosted-fallback.yml",
            WORKFLOWS / "gha-clone-server-contract.yml",
            WORKFLOWS / "gha-clone-images.yml",
            REGISTER,
            Path(__file__),
            DOC,
        ]
        token_pattern = re.compile(r"(?:ghp_|github_pat_)[A-Za-z0-9_]{20,}")
        for path in paths:
            if not path.is_file():
                continue
            with self.subTest(path=path.relative_to(ROOT)):
                self.assertIsNone(token_pattern.search(read(path)))


if __name__ == "__main__":
    unittest.main()
