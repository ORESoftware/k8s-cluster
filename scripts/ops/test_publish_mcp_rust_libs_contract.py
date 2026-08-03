#!/usr/bin/env python3
"""Static regression tests for the protected MCP repository publisher.

The suite is intentionally credential-free and mutation-free. It validates the
reviewed security and ordering contract of the trusted-main publisher scripts.
"""

from __future__ import annotations

import pathlib
import re
import sys
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
PUBLISHER = ROOT / "scripts/ops/publish_mcp_rust_libs.sh"
BROKER = ROOT / "scripts/ops/run_protected_mcp_rust_libs_publisher.sh"
WORKFLOW = ROOT / ".github/workflows/ops-publish-mcp-rust-libs.yml"


class PublisherContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.publisher = PUBLISHER.read_text(encoding="utf-8")
        cls.broker = BROKER.read_text(encoding="utf-8")
        cls.workflow = WORKFLOW.read_text(encoding="utf-8")

    def test_shells_fail_closed_and_use_private_umask(self) -> None:
        self.assertIn("set -Eeuo pipefail", self.publisher)
        self.assertIn("umask 077", self.publisher)
        self.assertIn("set -Eeuo pipefail", self.broker)
        self.assertNotIn("set +e", self.publisher)
        self.assertNotIn("set +e", self.broker)

    def test_source_and_target_are_exactly_pinned(self) -> None:
        required = (
            "readonly expected_login='ORESoftware'",
            "readonly target_repository='ORESoftware/mcp-rust-libs'",
            "readonly source_repository='ORESoftware/testing'",
            "readonly source_sha='069b1aa4251658c8348d2eb477ad71369d9b742b'",
            "readonly source_subdirectory='mcp-rust-libs'",
            "readonly source_manifest_sha256='b9ba89f29dca3e5020430d3a5d35967e523d3e94db9168a91cdf24a9bd5f2a33'",
        )
        for item in required:
            with self.subTest(item=item):
                self.assertIn(item, self.publisher)

    def test_child_accepts_only_stdin_broker_credential(self) -> None:
        self.assertIn("stage=receive-protected-credential", self.publisher)
        self.assertIn("IFS= read -r encoded_pat", self.publisher)
        self.assertIn("base64 --decode", self.publisher)
        self.assertNotIn("gh auth token", self.publisher)
        self.assertNotRegex(self.publisher, r"(?:ghp|github_pat)_[A-Za-z0-9_]+")
        self.assertNotRegex(self.broker, r"(?:ghp|github_pat)_[A-Za-z0-9_]+")

    def test_broker_preserves_ordered_credential_sources(self) -> None:
        positions = [
            self.broker.index("aws-secrets-manager"),
            self.broker.index("kubernetes-secret"),
            self.broker.index("protected-gh-cli"),
        ]
        self.assertEqual(positions, sorted(positions))
        self.assertIn("base64", self.broker)
        self.assertIn("runuser", self.broker)

    def test_github_api_is_bounded_and_does_not_follow_redirects(self) -> None:
        self.assertIn("urllib.request.urlopen(req, timeout=30)", self.publisher)
        self.assertIn('"X-GitHub-Api-Version": "2022-11-28"', self.publisher)
        self.assertNotIn("urlopen(req)", self.publisher)
        self.assertNotIn("curl -L", self.publisher)
        self.assertNotIn("curl --location", self.publisher)

    def test_validation_precedes_all_repository_mutation(self) -> None:
        order = (
            "stage=checkout-reviewed-source",
            "stage=validate-reviewed-source",
            "stage=ensure-target-repository",
            "stage=prepare-target-review-gate",
            "stage=publish-reviewed-source-branch",
            "stage=ensure-target-pull-request",
            "stage=complete",
        )
        positions = [self.publisher.index(marker) for marker in order]
        self.assertEqual(positions, sorted(positions))

    def test_publication_is_no_force_and_divergence_fails_closed(self) -> None:
        self.assertNotRegex(self.publisher, r"git[^\n]*push[^\n]*(?:--force|-f(?:\s|$))")
        self.assertIn("Refusing unexpected", self.publisher)
        self.assertIn("Refusing divergent", self.publisher)
        self.assertIn("test \"$(python3 \"$api_helper\" get-ref", self.publisher)

    def test_workflow_executes_only_checked_in_broker(self) -> None:
        self.assertIn("scripts/ops/run_protected_mcp_rust_libs_publisher.sh", self.workflow)
        self.assertIn("scripts/ops/publish_mcp_rust_libs.sh", self.workflow)
        self.assertIn("permissions:", self.workflow)
        self.assertIn("id-token: write", self.workflow)
        self.assertNotRegex(self.workflow, r"(?:ghp|github_pat)_[A-Za-z0-9_]+")


if __name__ == "__main__":
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(PublisherContractTests)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    sys.exit(0 if result.wasSuccessful() else 1)
