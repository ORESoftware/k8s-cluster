#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("validate_meta_agent_app_publisher.py")
SPEC = importlib.util.spec_from_file_location("meta_agent_app_publisher_contract", MODULE_PATH)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


class MetaAgentAppPublisherContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.text = MODULE.SCRIPT_PATH.read_text(encoding="utf-8")

    def assert_code(self, text: str, code: str) -> None:
        with self.assertRaises(MODULE.ContractError) as context:
            MODULE.validate_text(text)
        self.assertEqual(context.exception.code, code)

    def test_report_is_deterministic_and_exact(self) -> None:
        first = MODULE.validate_text(self.text)
        second = MODULE.validate_text(self.text)
        self.assertEqual(first, second)
        self.assertEqual(first["raw_curl_count"], 1)
        self.assertEqual(first["stage_count"], 12)
        self.assertEqual(first["required_guard_count"], len(MODULE.REQUIRED_SNIPPETS))
        self.assertTrue(first["creates_review_pull_request"])
        self.assertEqual(
            first["expected_heads"],
            {
                "refs/heads/main": "4d6ec3ad0ec7b688f0e777129eee7e0f0d999df1",
                "refs/heads/agent/den-1057-meta-agent-control-plane": "789d48039da232faed985d4f8de176959f117e08",
            },
        )

    def test_each_required_guard_fails_closed_when_removed(self) -> None:
        for code, snippet in MODULE.REQUIRED_SNIPPETS.items():
            with self.subTest(code=code):
                self.assertIn(snippet, self.text)
                self.assert_code(self.text.replace(snippet, "", 1), code)

    def test_each_sealed_constant_rejects_drift(self) -> None:
        for name, expected in MODULE.EXPECTED.items():
            with self.subTest(name=name):
                replacement = ("0" if expected[0] != "0" else "1") + expected[1:]
                self.assert_code(self.text.replace(expected, replacement, 1), "constant-drift")

    def test_direct_unbounded_curl_is_rejected(self) -> None:
        mutated = self.text.replace("github_curl \\\n", "curl \\\n", 1)
        self.assert_code(mutated, "unbounded-curl")

    def test_redirect_following_is_rejected(self) -> None:
        mutated = self.text.replace("--max-time 30 \\\n", "--max-time 30 --location \\\n", 1)
        self.assert_code(mutated, "redirect-following")

    def test_fail_open_shell_is_rejected(self) -> None:
        mutated = self.text.replace("umask 077", "umask 077\nset +e", 1)
        self.assert_code(mutated, "fail-open-shell")

    def test_pat_shaped_literal_is_rejected(self) -> None:
        mutated = self.text + "\n# forbidden example: ghp_abcdefghijklmnopqrstuvwxyz123456\n"
        self.assert_code(mutated, "literal-token")

    def test_stage_reordering_is_rejected(self) -> None:
        first = "stage=resolve-org-installation"
        second = "stage=mint-installation-token"
        mutated = self.text.replace(first, "stage=temporary-swap", 1)
        mutated = mutated.replace(second, first, 1)
        mutated = mutated.replace("stage=temporary-swap", second, 1)
        self.assert_code(mutated, "stage-order")

    def test_non_bash_shebang_is_rejected(self) -> None:
        self.assert_code(
            self.text.replace("#!/usr/bin/env bash", "#!/bin/sh", 1),
            "invalid-shebang",
        )

    def test_crlf_is_rejected(self) -> None:
        self.assert_code(self.text.replace("\n", "\r\n"), "invalid-newline")


if __name__ == "__main__":
    unittest.main()
