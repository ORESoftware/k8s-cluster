#!/usr/bin/env python3
"""Static contract for the trusted-main submodule App bootstrap workflow."""

from __future__ import annotations

import re
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/ops-bootstrap-k8s-submodule-app-secrets.yml"
ACTION = (
    "aws-actions/configure-aws-credentials@"
    "e6de054238d6b7531b4efff3b6587d9aade6a06c"
)
ROLE_CANDIDATES = (
    ("aws_bootstrap", "AWS_ROLE_BOOTSTRAP"),
    ("aws_primary", "AWS_ROLE_PRIMARY"),
    ("aws_remote_dev", "AWS_ROLE_REMOTE_DEV"),
    ("aws_oidc", "AWS_ROLE_OIDC"),
    ("aws_ecr", "AWS_ROLE_ECR"),
)


class BootstrapWorkflowContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.text = WORKFLOW.read_text(encoding="utf-8")

    def test_trusted_main_and_oidc_permissions_remain_fail_closed(self) -> None:
        self.assertIn("id-token: write", self.text)
        self.assertIn(
            "if: github.repository == 'ORESoftware/k8s-cluster' && "
            "github.ref == 'refs/heads/main'",
            self.text,
        )
        self.assertNotIn("if: secrets.", self.text)
        self.assertNotIn("pull_request_target:", self.text)

    def test_approved_role_candidates_are_declared_without_values(self) -> None:
        expected = {
            "AWS_ROLE_BOOTSTRAP": "secrets.K8S_SUBMODULE_BOOTSTRAP_ROLE_ARN",
            "AWS_ROLE_PRIMARY": "secrets.AWS_ROLE_TO_ASSUME",
            "AWS_ROLE_REMOTE_DEV": "secrets.REMOTE_DEV_AWS_ROLE_TO_ASSUME",
            "AWS_ROLE_OIDC": "secrets.AWS_OIDC_ROLE_ARN",
            "AWS_ROLE_ECR": "secrets.AWS_ECR_ROLE_ARN",
        }
        for variable, expression in expected.items():
            with self.subTest(variable=variable):
                self.assertIn(
                    f"{variable}: ${{{{ {expression} }}}}",
                    self.text,
                )

    def test_role_attempts_are_bounded_ordered_and_masked(self) -> None:
        self.assertEqual(self.text.count(f"uses: {ACTION}"), len(ROLE_CANDIDATES))
        offsets: list[int] = []
        for index, (step_id, role_variable) in enumerate(ROLE_CANDIDATES):
            marker = f"id: {step_id}"
            offset = self.text.index(marker)
            offsets.append(offset)
            next_offset = (
                self.text.index(
                    f"id: {ROLE_CANDIDATES[index + 1][0]}", offset + len(marker)
                )
                if index + 1 < len(ROLE_CANDIDATES)
                else self.text.index("id: assert_aws_oidc", offset + len(marker))
            )
            block = self.text[offset:next_offset]
            self.assertIn("continue-on-error: true", block)
            self.assertIn(f"uses: {ACTION}", block)
            self.assertIn(
                f"role-to-assume: ${{{{ env.{role_variable} }}}}", block
            )
            self.assertIn("unset-current-credentials: true", block)
            self.assertIn("mask-aws-account-id: true", block)
            if index > 0:
                for earlier_id, _ in ROLE_CANDIDATES[:index]:
                    self.assertIn(
                        f"steps.{earlier_id}.outcome != 'success'", block
                    )
        self.assertEqual(offsets, sorted(offsets))

    def test_no_single_expression_can_shadow_later_roles(self) -> None:
        for line in self.text.splitlines():
            if "role-to-assume:" not in line:
                continue
            self.assertNotIn("||", line)
            self.assertNotIn("secrets.", line)

    def test_success_assertion_precedes_secret_hydration(self) -> None:
        assertion = self.text.index("id: assert_aws_oidc")
        hydration = self.text.index(
            "name: Hydrate repository secrets through the protected administration host"
        )
        self.assertLess(assertion, hydration)
        block = self.text[assertion:hydration]
        self.assertIn("if: always()", block)
        self.assertIn("approved AWS OIDC roles accepted this workflow", block)
        self.assertRegex(
            block,
            re.compile(r"for entry in[\s\S]+exit 0[\s\S]+exit 1"),
        )

    def test_personal_token_is_not_a_submodule_recovery_path(self) -> None:
        self.assertIn(".pat_used_for_submodule_access == false", self.text)
        self.assertNotIn("SUPERPROJECT_READ_TOKEN", self.text)
        self.assertNotIn("K8S_LIBS_DEPLOY_KEY", self.text)


if __name__ == "__main__":
    unittest.main()
